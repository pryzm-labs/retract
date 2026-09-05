use super::{
    ArchiveError, ArchiveSearch, ArchiveStore,
    model::ImportBatch,
    test_support::{Fixture, account, attachment, batch, envelope, fts_count, registered, source},
};
use crate::providers::ports::{ConversationQuery, ResolveRequest};
use retract_domain::{
    ContentKind, ContentRecord, EvidenceState, PrivacyKind, Scope, ScopedResourceRef,
};

fn request(text: &str) -> ArchiveSearch {
    ArchiveSearch {
        scope: source().scope(),
        text: text.into(),
        kinds: vec![],
        author: None,
        before: None,
        after: None,
        cursor: None,
        limit: 200,
    }
}

fn publish(store: &mut ArchiveStore, scope: &Scope, inputs: Vec<ImportBatch>) {
    let session = store.begin_import(scope).unwrap();
    for (sequence, input) in inputs.into_iter().enumerate() {
        store
            .append_batch(&session, sequence as u64, input)
            .unwrap();
    }
    store.finish_import(&session).unwrap();
}

fn ids(items: &[ContentRecord]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item.resource.canonical_key.as_str())
        .collect()
}

#[test]
fn literal_search_is_inert_and_returns_complete_selected_observation() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut first = batch("a", "alpha bright sky quoted \"words\" x-ray");
    first.contents[0].provider_metadata = Some(envelope("retained".into()));
    first.contents[0]
        .attachments
        .push(attachment("needle query@example.test"));
    publish(
        &mut store,
        &source().scope(),
        vec![first, batch("b", "alpha separated bright distant sky")],
    );
    for (text, expected) in [
        ("bright sky", vec!["a"]),
        ("quoted \"words\"", vec!["a"]),
        ("x-ray", vec!["a"]),
        ("needle", vec!["a"]),
        ("\" OR alpha --", vec![]),
        ("'); DROP TABLE sources; --", vec![]),
    ] {
        let page = store.search(request(text)).unwrap();
        assert_eq!(ids(&page.items), expected);
    }
    let item = store.search(request("needle")).unwrap().items.remove(0);
    assert_eq!(item.evidence, EvidenceState::Archive);
    assert_eq!(item.provider_metadata, Some(envelope("retained".into())));
    assert_eq!(
        item.attachments[0].safe_display_name.as_deref(),
        Some("needle query@example.test")
    );
    assert!(item.privacy_findings.contains(&PrivacyKind::EmailAddress));
    assert!(item.detector_version.is_some());

    let mut second_source = source();
    second_source.id = uuid::Uuid::from_u128(7).try_into().unwrap();
    second_source.archive_fingerprint = Some("other-export".into());
    store
        .register_source(account(), second_source.clone())
        .unwrap();
    let mut newer = batch("a", "beta newer edit");
    for item in &mut newer.contents {
        item.scope = second_source.scope();
        item.observed_at = "2026-09-06T00:00:00Z".parse().unwrap();
    }
    for item in &mut newer.conversations {
        item.scope = second_source.scope();
    }
    for item in &mut newer.actors {
        item.scope = second_source.scope();
    }
    publish(&mut store, &second_source.scope(), vec![newer]);
    assert_eq!(
        ids(&store.search(request("bright sky")).unwrap().items),
        ["a"]
    );
    assert!(store.search(request("beta")).unwrap().items.is_empty());
    let alpha = store.search(request("alpha")).unwrap();
    let mut alpha_ids = ids(&alpha.items);
    alpha_ids.sort();
    assert_eq!(alpha_ids, ["a", "b"]);
    let resolved = store
        .resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![ScopedResourceRef {
                scope: source().scope(),
                id: *item.id.as_uuid(),
                resource: item.resource.clone(),
            }],
        })
        .unwrap();
    assert_eq!(
        resolved[0].searchable_text,
        "alpha bright sky quoted \"words\" x-ray"
    );
    let mut query = request("beta");
    query.scope = second_source.scope();
    assert_eq!(ids(&store.search(query).unwrap().items), ["a"]);
    for foreign in [
        Scope {
            account_id: uuid::Uuid::from_u128(8).try_into().unwrap(),
            ..source().scope()
        },
        Scope {
            provider: "foreign".to_owned().try_into().unwrap(),
            ..source().scope()
        },
    ] {
        let mut query = request("alpha");
        query.scope = foreign;
        assert_eq!(store.search(query), Err(ArchiveError::ScopeMismatch));
    }
}

#[test]
fn incomplete_sources_are_hidden_by_every_query_entry_point() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    store
        .append_batch(&session, 0, batch("a", "alpha"))
        .unwrap();
    assert_eq!(
        store.search(request("alpha")),
        Err(ArchiveError::IncompleteSource)
    );
    assert_eq!(
        store.list_conversations(ConversationQuery {
            scope: source().scope(),
            cursor: None,
            limit: 10
        }),
        Err(ArchiveError::IncompleteSource)
    );
    assert_eq!(
        store.resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![]
        }),
        Err(ArchiveError::IncompleteSource)
    );
}

#[test]
fn equal_timestamp_pages_and_filter_bound_cursors_are_stable() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    publish(
        &mut store,
        &source().scope(),
        vec![
            batch("a", "alpha"),
            batch("b", "alpha"),
            batch("c", "alpha"),
        ],
    );
    let mut query = request("alpha");
    query.limit = 2;
    let first = store.search(query.clone()).unwrap();
    assert_eq!(first.items.len(), 2);
    query.cursor = first.next_cursor.clone();
    let second = store.search(query.clone()).unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    let mut all = first.items;
    all.extend(second.items);
    let mut found = ids(&all);
    found.sort();
    assert_eq!(found, ["a", "b", "c"]);
    assert!(all.windows(2).all(|pair| pair[0].id > pair[1].id));
    query.text = "beta".into();
    assert_eq!(store.search(query.clone()), Err(ArchiveError::StaleCursor));
    query.text = "alpha".into();
    query.scope.source_id = uuid::Uuid::from_u128(9).try_into().unwrap();
    assert_eq!(store.search(query), Err(ArchiveError::StaleCursor));
}

#[test]
fn filters_preserve_exact_fractional_timestamps_and_reject_invalid_bounds() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut input = batch("a", "alpha");
    input.contents[0].timestamp = "2026-09-05T00:00:00.000000001Z".parse().unwrap();
    let mut other = batch("b", "alpha");
    other.contents[0].kind = ContentKind::Document;
    other.contents[0].timestamp = "2026-09-05T00:00:00.000000002Z".parse().unwrap();
    publish(&mut store, &source().scope(), vec![input.clone(), other]);
    let mut query = request("");
    query.after = Some("2026-09-05T00:00:00.000000001Z".parse().unwrap());
    assert_eq!(ids(&store.search(query.clone()).unwrap().items), ["b"]);
    query.after = None;
    query.before = Some("2026-09-05T00:00:00.000000002Z".parse().unwrap());
    assert_eq!(ids(&store.search(query.clone()).unwrap().items), ["a"]);
    query.before = None;
    query.kinds = vec![ContentKind::Document];
    query.author = Some(input.contents[0].author_id);
    assert_eq!(ids(&store.search(query.clone()).unwrap().items), ["b"]);
    query.author = Some(uuid::Uuid::from_u128(10).try_into().unwrap());
    assert!(store.search(query).unwrap().items.is_empty());
    let mut query = request("");
    query.after = Some(input.contents[0].timestamp);
    query.before = query.after;
    assert_eq!(store.search(query), Err(ArchiveError::InvalidRecord));
    for limit in [0, 201] {
        let mut query = request("");
        query.limit = limit;
        assert_eq!(store.search(query), Err(ArchiveError::LimitExceeded));
    }
    assert_eq!(
        store.search(request(&"x".repeat(4097))),
        Err(ArchiveError::LimitExceeded)
    );
}

#[test]
fn resolve_validates_scope_locators_and_limits_before_lookup() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let input = batch("a", "alpha");
    let target = ScopedResourceRef {
        id: *input.contents[0].id.as_uuid(),
        scope: source().scope(),
        resource: input.contents[0].resource.clone(),
    };
    publish(&mut store, &source().scope(), vec![input]);
    assert_eq!(
        store
            .resolve(ResolveRequest {
                scope: source().scope(),
                refs: vec![target.clone(); 200]
            })
            .unwrap()
            .len(),
        200
    );
    assert_eq!(
        store.resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![target.clone(); 201]
        }),
        Err(ArchiveError::LimitExceeded)
    );
    let mut foreign = target.clone();
    foreign.scope.source_id = uuid::Uuid::from_u128(11).try_into().unwrap();
    assert_eq!(
        store.resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![foreign]
        }),
        Err(ArchiveError::ScopeMismatch)
    );
    let mut oversized = target.clone();
    oversized.resource.locator_payload = serde_json::json!({"nativeId": "x".repeat(65536)});
    assert_eq!(
        store.resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![oversized]
        }),
        Err(ArchiveError::LimitExceeded)
    );
    let mut forged = target.clone();
    forged.resource.locator_payload = serde_json::json!({"nativeId": "different"});
    assert_eq!(
        store.resolve(ResolveRequest {
            scope: source().scope(),
            refs: vec![forged]
        }),
        Err(ArchiveError::InvalidRecord)
    );
    assert_eq!(
        ids(&store
            .resolve(ResolveRequest {
                scope: source().scope(),
                refs: vec![target]
            })
            .unwrap()),
        ["a"]
    );
}

#[test]
fn updates_and_synthetic_removal_delete_fts_terms_and_invalidate_generation() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut updated = batch("a", "newterm");
    updated.contents[0]
        .attachments
        .push(attachment("needle query@example.test"));
    publish(
        &mut store,
        &source().scope(),
        vec![batch("a", "oldterm"), updated, batch("b", "newterm")],
    );
    assert!(store.search(request("oldterm")).unwrap().items.is_empty());
    let mut query = request("newterm");
    query.limit = 1;
    query.cursor = store.search(query.clone()).unwrap().next_cursor;
    store
        .transaction(|tx| {
            tx.execute(
                "DELETE FROM sources WHERE source_id = ?",
                [source().id.as_uuid().to_string()],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(store.search(query.clone()), Err(ArchiveError::StaleCursor));
    assert_eq!(fts_count(&store, "needle"), 0);
    assert_eq!(fts_count(&store, "query"), 0);
    store.register_source(account(), source()).unwrap();
    publish(
        &mut store,
        &source().scope(),
        vec![batch("a", "newterm"), batch("b", "newterm")],
    );
    assert_eq!(store.search(query), Err(ArchiveError::StaleCursor));
}

#[test]
fn filtered_recent_reads_use_scope_filter_and_order_indexes() {
    let fixture = Fixture::new();
    let store = registered(&fixture);
    let mut plans = Vec::new();
    for (predicate, constraint) in [
        ("author_id=?4", "author_id=?"),
        ("json_extract(record_json, '$.kind')=?4", "<expr>=?"),
    ] {
        let details = store.transaction(|tx| {
            let sql = format!("EXPLAIN QUERY PLAN SELECT record_json FROM content_observations WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND {predicate} ORDER BY timestamp_seconds DESC, timestamp_nanos DESC, resource_id DESC LIMIT 201");
            Ok(tx.prepare(&sql).unwrap().query_map(["synthetic", "account", "source", "filter"], |row| row.get::<_, String>(3)).unwrap().collect::<Result<Vec<_>, _>>().unwrap())
        }).unwrap().join("; ");
        println!("filtered query plan: {details}");
        plans.push((details, constraint));
    }
    for (details, constraint) in plans {
        assert!(details.contains("provider=? AND account_id=? AND source_id=?"));
        assert!(details.contains(constraint), "{details}");
        assert!(!details.contains("TEMP B-TREE"), "{details}");
    }
}

#[test]
fn strict_cursors_reject_unknown_fields_versions_and_wrong_query_types() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    publish(
        &mut store,
        &source().scope(),
        vec![batch("a", "alpha"), batch("b", "alpha")],
    );
    let mut query = request("alpha");
    query.limit = 1;
    let token = store.search(query.clone()).unwrap().next_cursor.unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&token).unwrap()).unwrap();
    for (field, replacement) in [
        ("version", serde_json::json!(2)),
        ("unexpected", serde_json::json!("inert")),
        ("revision", serde_json::json!(999)),
        ("timestamp", serde_json::Value::Null),
        (
            "run",
            serde_json::json!("00000000-0000-0000-0000-000000000001"),
        ),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement;
        query.cursor = Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&changed).unwrap()));
        assert_eq!(store.search(query.clone()), Err(ArchiveError::StaleCursor));
    }
    for malformed in ["?".into(), "x".repeat(4097)] {
        query.cursor = Some(malformed);
        assert_eq!(store.search(query.clone()), Err(ArchiveError::StaleCursor));
    }
    assert_eq!(
        store.list_conversations(ConversationQuery {
            scope: source().scope(),
            limit: 1,
            cursor: Some(token)
        }),
        Err(ArchiveError::StaleCursor)
    );
}

#[test]
fn conversations_page_complete_source_records_and_content_pages_preserve_leap_seconds() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut first = batch("a", "alpha");
    first.contents[0].timestamp = "2016-12-31T23:59:60.500000000Z".parse().unwrap();
    first.conversations[0].provider_metadata = Some(envelope("room evidence".into()));
    let mut second = batch("b", "alpha");
    second.contents[0].timestamp = "2017-01-01T00:00:00Z".parse().unwrap();
    second.conversations[0].resource.canonical_key = "second-room".into();
    second.conversations[0].resource.locator_payload =
        serde_json::json!({"nativeId": "second-room"});
    second.conversations[0].id = second.conversations[0]
        .resource
        .resource_id()
        .unwrap()
        .try_into()
        .unwrap();
    second.contents[0].conversation_id = second.conversations[0].id;
    publish(&mut store, &source().scope(), vec![first, second]);
    let mut query = request("");
    query.limit = 1;
    let page = store.search(query.clone()).unwrap();
    assert_eq!(ids(&page.items), ["b"]);
    query.cursor = page.next_cursor;
    let page = store.search(query).unwrap();
    assert_eq!(ids(&page.items), ["a"]);
    assert_eq!(
        page.items[0].timestamp.timestamp_subsec_nanos(),
        1_500_000_000
    );
    let mut query = ConversationQuery {
        scope: source().scope(),
        cursor: None,
        limit: 1,
    };
    let first = store.list_conversations(query.clone()).unwrap();
    let token = first.next_cursor.clone().unwrap();
    let mut cursor: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&token).unwrap()).unwrap();
    cursor.as_object_mut().unwrap().remove("timestamp");
    query.cursor = Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap()));
    assert_eq!(
        store.list_conversations(query.clone()),
        Err(ArchiveError::StaleCursor)
    );
    query.cursor = Some(token);
    let second = store.list_conversations(query).unwrap();
    assert!(second.next_cursor.is_none());
    let mut rooms = first.items;
    rooms.extend(second.items);
    let mut names: Vec<_> = rooms
        .iter()
        .map(|room| room.resource.canonical_key.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["room", "second-room"]);
    assert_eq!(
        rooms
            .iter()
            .find(|room| room.resource.canonical_key == "room")
            .unwrap()
            .provider_metadata,
        Some(envelope("room evidence".into()))
    );
}

#[test]
fn maximum_page_is_bounded_and_filter_order_normalizes_cursor_digest() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut input = batch("item-000", "alpha");
    for index in 1..201 {
        input.contents.push(
            batch(&format!("item-{index:03}"), "alpha")
                .contents
                .remove(0),
        );
    }
    publish(&mut store, &source().scope(), vec![input]);
    let mut query = request(" alpha ");
    query.kinds = vec![ContentKind::Text, ContentKind::Document];
    let first = store.search(query.clone()).unwrap();
    assert_eq!(first.items.len(), 200);
    query.cursor = first.next_cursor;
    query.text = "alpha".into();
    query.kinds = vec![ContentKind::Document, ContentKind::Text, ContentKind::Text];
    let second = store.search(query).unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    assert!(first.items.iter().all(|item| item.id != second.items[0].id));
    let hostile = "synthetic-secret@example.test".repeat(200);
    let error = store.search(request(&hostile)).unwrap_err();
    assert_eq!(error.to_string(), "limit_exceeded");
    assert_eq!(format!("{error:?}"), "LimitExceeded");
}
