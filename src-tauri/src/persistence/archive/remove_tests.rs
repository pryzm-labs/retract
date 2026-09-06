use std::{fs, os::unix::fs::PermissionsExt};

use retract_domain::{Scope, SourceRecord};
use rusqlite::Connection;

use super::{
    ArchiveError, ArchiveSearch, ArchiveStore,
    model::{ImportBatch, RemovalOutcome},
    test_support::{Fixture, account, attachment, batch, envelope, fts_count, registered, source},
};

fn request(scope: &Scope) -> ArchiveSearch {
    ArchiveSearch {
        scope: scope.clone(),
        text: String::new(),
        kinds: vec![],
        author: None,
        before: None,
        after: None,
        cursor: None,
        limit: 200,
    }
}

fn publish(store: &mut ArchiveStore, source: &SourceRecord, inputs: Vec<ImportBatch>) {
    let session = store.begin_import(&source.scope()).unwrap();
    for (sequence, mut input) in inputs.into_iter().enumerate() {
        for item in &mut input.contents {
            item.scope = source.scope();
        }
        for item in &mut input.actors {
            item.scope = source.scope();
        }
        for item in &mut input.conversations {
            item.scope = source.scope();
        }
        store
            .append_batch(&session, sequence as u64, input)
            .unwrap();
    }
    store.finish_import(&session).unwrap();
}

fn other_source() -> SourceRecord {
    let mut other = source();
    other.id = uuid::Uuid::from_u128(77).try_into().unwrap();
    other.archive_fingerprint = Some("other-synthetic-export".into());
    other
}

fn scoped_rows(store: &ArchiveStore, scope: &Scope) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    store.transaction(|tx| {
        Ok(["sources", "conversation_observations", "actor_observations", "content_observations",
            "attachments", "privacy_findings", "import_runs", "import_batch_receipts", "import_warnings"]
            .into_iter().map(|table| {
                let mut query = tx.prepare(&format!("SELECT * FROM {table} WHERE provider=?1 AND account_id=?2 AND source_id=?3 ORDER BY rowid")).unwrap();
                let columns = query.column_count();
                query.query_map(super::ingest_state::scope_sql(scope), |row| {
                    (0..columns).map(|column| row.get(column)).collect::<rusqlite::Result<Vec<_>>>()
                }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
            }).collect())
    }).unwrap()
}

#[test]
fn removing_one_snapshot_preserves_other_full_records_and_export() {
    let fixture = Fixture::new();
    let sentinel = fixture.path.parent().unwrap().join("original-export.txt");
    fs::write(&sentinel, b"synthetic export stays untouched").unwrap();
    let mut store = registered(&fixture);
    let mut first = batch("shared", "erasedcanary first@example.test");
    first.contents[0]
        .attachments
        .push(attachment("erasedattachment"));
    publish(
        &mut store,
        &source(),
        vec![first, batch("only-a", "erasedcanary")],
    );
    let other = other_source();
    store.register_source(account(), other.clone()).unwrap();
    let mut second = batch("shared", "surviving observation other@example.test");
    second.contents[0]
        .attachments
        .push(attachment("retainedattachment"));
    second.contents[0].provider_metadata = Some(envelope("retained metadata".into()));
    second.actors[0].display_name = "Other actor observation".into();
    second.conversations[0].title = "Other room observation".into();
    publish(&mut store, &other, vec![second]);
    let expected = scoped_rows(&store, &other.scope());
    let expected_items = store.search(request(&other.scope())).unwrap();
    let mut stale_query = request(&source().scope());
    stale_query.limit = 1;
    stale_query.cursor = store.search(stale_query.clone()).unwrap().next_cursor;
    assert_eq!(
        store.remove_source(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 2,
            maintenance_pending: false
        })
    );
    assert_eq!(store.search(stale_query), Err(ArchiveError::StaleCursor));
    assert_eq!(fts_count(&store, "erasedcanary"), 0);
    assert_eq!(fts_count(&store, "erasedattachment"), 0);
    assert_eq!(fts_count(&store, "retainedattachment"), 1);
    // Shared primary observations and content references duplicate IDs in the
    // membership input; absent parent/reply/thread values must not poison NOT IN.
    let retained: Vec<String> = store
        .transaction(|tx| {
            Ok(tx
                .prepare("SELECT canonical_key FROM resource_identities ORDER BY canonical_key")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap())
        })
        .unwrap();
    assert_eq!(retained, ["author", "room", "shared"]);
    assert_eq!(scoped_rows(&store, &other.scope()), expected);
    assert!(
        scoped_rows(&store, &source().scope())
            .iter()
            .all(Vec::is_empty)
    );
    drop(store);
    let mut store = fixture.open();
    assert_eq!(
        store.search(request(&other.scope())).unwrap(),
        expected_items
    );
    assert_eq!(scoped_rows(&store, &other.scope()), expected);
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 2,
            maintenance_pending: false
        })
    );
    assert_eq!(
        store.register_source(account(), source()),
        Err(ArchiveError::InvalidRecord)
    );
    assert_eq!(
        fs::read(sentinel).unwrap(),
        b"synthetic export stays untouched"
    );
    // A fresh source UUID is a fresh import and never inherits the old receipt.
    let mut fresh = source();
    fresh.id = uuid::Uuid::from_u128(78).try_into().unwrap();
    store.register_source(account(), fresh.clone()).unwrap();
    publish(
        &mut store,
        &fresh,
        vec![batch("shared", "fresh imported observation")],
    );
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 2,
            maintenance_pending: false
        })
    );
    assert_eq!(
        store.search(request(&fresh.scope())).unwrap().items[0].searchable_text,
        "fresh imported observation"
    );
    assert_eq!(scoped_rows(&store, &other.scope()), expected);
}

#[test]
fn failed_maintenance_stays_removed_and_retry_keeps_original_count_after_restart() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    store
        .append_batch(&session, 0, batch("a", "erasedcanary"))
        .unwrap();
    let checkpoint = super::test_support::checkpoint(&store);
    let parent = fixture.path.parent().unwrap().to_owned();
    let blocked = parent.clone();
    store.before_maintenance = Some(Box::new(move |_| {
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).unwrap();
        Ok(())
    }));
    let result = store.remove_source(&source().scope());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        result,
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: true
        })
    );
    store.before_maintenance = None;
    assert_eq!(fts_count(&store, "erasedcanary"), 0);
    assert_eq!(
        store.append_batch(&session, 1, batch("b", "cannot restore")),
        Err(ArchiveError::ScopeMismatch)
    );
    assert!(matches!(
        store.retry_import(&checkpoint),
        Err(ArchiveError::StaleCursor)
    ));
    assert_eq!(
        store.search(request(&source().scope())),
        Err(ArchiveError::ScopeMismatch)
    );
    assert_eq!(
        store.register_source(account(), source()),
        Err(ArchiveError::InvalidRecord)
    );
    drop(store);
    let mut store = fixture.open();
    store.before_maintenance = Some(Box::new(|_| Err(ArchiveError::StorageFailure)));
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: true
        })
    );
    store.before_maintenance = None;
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: false
        })
    );
    assert_eq!(
        store.remove_source(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: false
        })
    );
    drop(store);
    let mut store = fixture.open();
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: false
        })
    );
}

#[test]
fn wrong_scope_and_unknown_source_cannot_remove_or_complete_another_receipt() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    publish(&mut store, &source(), vec![batch("a", "alpha")]);
    for wrong in [
        Scope {
            provider: "foreign".to_owned().try_into().unwrap(),
            ..source().scope()
        },
        Scope {
            account_id: uuid::Uuid::from_u128(88).try_into().unwrap(),
            ..source().scope()
        },
    ] {
        assert_eq!(
            store.remove_source(&wrong),
            Err(ArchiveError::ScopeMismatch)
        );
        assert_eq!(
            store.retry_cleanup(&wrong),
            Err(ArchiveError::ScopeMismatch)
        );
    }
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Err(ArchiveError::InvalidRecord)
    );
    assert_eq!(
        store.remove_source(&other_source().scope()),
        Ok(RemovalOutcome {
            removed_items: 0,
            maintenance_pending: false
        })
    );
    store.before_maintenance = Some(Box::new(|_: &Connection| Err(ArchiveError::StorageFailure)));
    assert_eq!(
        store
            .remove_source(&source().scope())
            .unwrap()
            .removed_items,
        1
    );
    for wrong in [
        Scope {
            provider: "foreign".to_owned().try_into().unwrap(),
            ..source().scope()
        },
        Scope {
            account_id: uuid::Uuid::from_u128(88).try_into().unwrap(),
            ..source().scope()
        },
    ] {
        assert_eq!(
            store.remove_source(&wrong),
            Err(ArchiveError::ScopeMismatch)
        );
        assert_eq!(
            store.retry_cleanup(&wrong),
            Err(ArchiveError::ScopeMismatch)
        );
    }
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: true
        })
    );
}

#[test]
fn surviving_references_keep_identities_without_primary_observations() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut input = batch("reply-root", "a removed observation");
    for native in ["thread-root", "parent-root"] {
        let mut conversation = input.conversations[0].clone();
        conversation.resource.canonical_key = native.into();
        conversation.resource.locator_payload = serde_json::json!({"nativeId": native});
        conversation.id = conversation
            .resource
            .resource_id()
            .unwrap()
            .try_into()
            .unwrap();
        input.conversations.push(conversation);
    }
    let mut survivor = batch("survivor", "retained body");
    survivor.actors.clear();
    survivor.contents[0].reply_to = Some(input.contents[0].id);
    survivor.contents[0].thread_parent = Some(input.conversations[1].id);
    let child = &mut survivor.conversations[0];
    child.resource.canonical_key = "child".into();
    child.resource.locator_payload = serde_json::json!({"nativeId": "child"});
    child.id = child.resource.resource_id().unwrap().try_into().unwrap();
    child.parent_id = Some(input.conversations[2].id);
    input
        .contents
        .push(batch("unreferenced", "erased orphan").contents.remove(0));
    publish(&mut store, &source(), vec![input]);
    let other = other_source();
    store.register_source(account(), other.clone()).unwrap();
    for item in &mut survivor.contents {
        item.scope = other.scope();
    }
    for item in &mut survivor.conversations {
        item.scope = other.scope();
    }
    let session = store.begin_import(&other.scope()).unwrap();
    store.append_batch(&session, 0, survivor).unwrap();
    let before = scoped_rows(&store, &other.scope());
    assert_eq!(
        store
            .remove_source(&source().scope())
            .unwrap()
            .removed_items,
        2
    );
    let keys: Vec<String> = store
        .transaction(|tx| {
            Ok(tx
                .prepare("SELECT canonical_key FROM resource_identities ORDER BY canonical_key")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap())
        })
        .unwrap();
    assert_eq!(
        keys,
        [
            "author",
            "child",
            "parent-root",
            "reply-root",
            "room",
            "survivor",
            "thread-root"
        ]
    );
    assert_eq!(scoped_rows(&store, &other.scope()), before);
    assert_eq!(
        store.remove_source(&other.scope()).unwrap().removed_items,
        1
    );
    assert_eq!(
        store
            .transaction(|tx| tx
                .query_row("SELECT count(*) FROM resource_identities", [], |row| row
                    .get::<_, i64>(0))
                .map_err(super::ingest_state::storage))
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .transaction(|tx| tx
                .query_row("SELECT count(*) FROM accounts", [], |row| row
                    .get::<_, i64>(0))
                .map_err(super::ingest_state::storage))
            .unwrap(),
        0
    );
}

#[test]
fn logical_removal_and_completion_commit_failures_leave_truthful_receipts() {
    // Real SQLite commit-hook rejection rolls back the transaction, including
    // FTS and cascades; the hook holds no pointer or synthetic data.
    unsafe extern "C" fn reject_commit(_: *mut std::ffi::c_void) -> std::ffi::c_int {
        1
    }
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    publish(&mut store, &source(), vec![batch("a", "erasedcanary")]);
    let before = scoped_rows(&store, &source().scope());
    {
        let connection = store.connection.lock().unwrap();
        // SAFETY: the connection remains live; the callback retains no data.
        unsafe {
            rusqlite::ffi::sqlite3_commit_hook(
                connection.handle(),
                Some(reject_commit),
                std::ptr::null_mut(),
            );
        }
    }
    assert_eq!(
        store.remove_source(&source().scope()),
        Err(ArchiveError::StorageFailure)
    );
    {
        let connection = store.connection.lock().unwrap();
        // SAFETY: removes the hook from this live fixture connection.
        unsafe {
            rusqlite::ffi::sqlite3_commit_hook(connection.handle(), None, std::ptr::null_mut());
        }
    }
    assert_eq!(scoped_rows(&store, &source().scope()), before);
    assert_eq!(fts_count(&store, "erasedcanary"), 1);
    store.transaction(|tx| {
        tx.execute_batch("CREATE TEMP TRIGGER reject_completion BEFORE UPDATE ON cleanup_tasks BEGIN SELECT RAISE(ABORT, 'synthetic completion failure'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert_eq!(
        store.remove_source(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: true
        })
    );
    assert_eq!(fts_count(&store, "erasedcanary"), 0);
    store
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_completion").unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: false
        })
    );
}

#[test]
fn wal_cleanup_erases_actual_fts_terms_and_keeps_every_artifact_encrypted() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    store
        .connection
        .lock()
        .unwrap()
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let mut input = batch("a", "erasedcanary first@example.test");
    input.contents[0]
        .attachments
        .push(attachment("erasedattachment"));
    publish(&mut store, &source(), vec![input]);
    store.before_maintenance = Some(Box::new(|_| Err(ArchiveError::StorageFailure)));
    assert!(
        store
            .remove_source(&source().scope())
            .unwrap()
            .maintenance_pending
    );
    store.transaction(|tx| {
        tx.execute_batch("CREATE VIRTUAL TABLE temp.terms USING fts5vocab(main, content_fts, row)").unwrap();
        let terms: i64 = tx.query_row("SELECT count(*) FROM temp.terms WHERE term IN ('erasedcanary', 'erasedattachment')", [], |row| row.get(0)).unwrap();
        assert_eq!(terms, 0);
        let blocks: Vec<Vec<u8>> = tx.prepare("SELECT block FROM content_fts_data").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(blocks.iter().all(|block| !block.windows(12).any(|part| part == b"erasedcanary")));
        tx.execute_batch("DROP TABLE temp.terms").unwrap();
        Ok(())
    }).unwrap();
    for (_, bytes) in super::test_support::snapshot_recovery_files(&fixture.path) {
        if let Some(bytes) = bytes {
            assert!(!bytes.windows(12).any(|part| part == b"erasedcanary"));
            assert!(!bytes.windows(16).any(|part| part == b"erasedattachment"));
        }
    }
    store.before_maintenance = None;
    assert!(
        !store
            .retry_cleanup(&source().scope())
            .unwrap()
            .maintenance_pending
    );
    let connection = store.connection.lock().unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "secure_delete", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT v FROM content_fts_config WHERE k='secure-delete'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn cleanup_receipt_count_scope_and_completed_state_cannot_be_rewritten() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    publish(&mut store, &source(), vec![batch("a", "body")]);
    store.remove_source(&source().scope()).unwrap();
    store
        .transaction(|tx| {
            for sql in [
                "UPDATE cleanup_tasks SET removed_items=99",
                "UPDATE cleanup_tasks SET task_id='other'",
                "UPDATE cleanup_tasks SET state='pending'",
                "UPDATE cleanup_tasks SET provider='other'",
                "UPDATE cleanup_tasks SET source_id='other'",
                "UPDATE cleanup_tasks SET account_id='other'",
            ] {
                assert!(tx.execute(sql, []).is_err());
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.retry_cleanup(&source().scope()),
        Ok(RemovalOutcome {
            removed_items: 1,
            maintenance_pending: false
        })
    );
}

#[test]
fn identity_pruning_materializes_reference_set_and_indexes_foreign_key_probes() {
    let fixture = Fixture::new();
    let store = registered(&fixture);
    let plan: Vec<String> = store
        .transaction(|tx| {
            Ok(tx
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {}",
                    super::remove::PRUNE_IDENTITIES
                ))
                .unwrap()
                .query_map(
                    ["synthetic", "11111111-1111-4111-8111-111111111111"],
                    |row| row.get(3),
                )
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap())
        })
        .unwrap();
    println!("identity-prune query plan:\n{}", plan.join("\n"));
    assert!(plan.iter().any(|detail| detail.contains("LIST SUBQUERY")));
    assert!(plan.iter().all(|detail| !detail.contains("CORRELATED")));
    for table in [
        "content_observations",
        "actor_observations",
        "conversation_observations",
    ] {
        assert!(
            plan.iter()
                .any(|detail| detail.contains(table) && detail.contains("resource_id=?")),
            "missing indexed FK probe for {table}"
        );
    }
}
