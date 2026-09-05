use retract_domain::{ContentRecord, EvidenceState};
use serde_json::json;
use uuid::Uuid;

use super::{
    ArchiveError, ArchiveStore,
    model::{ImportBatch, ImportPhase},
    test_support::{
        Fixture, account, attachment, batch, checkpoint, envelope, fts_count, registered, resource,
        source, sql_snapshot, stored_texts,
    },
};

#[test]
fn incomplete_sources_remain_unavailable_until_final_validation() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    assert_eq!(
        store.require_ready(&source().scope()),
        Err(ArchiveError::IncompleteSource)
    );
    let session = store.begin_import(&source().scope()).unwrap();
    store
        .append_batch(&session, 0, batch("first", "firstbody"))
        .unwrap();
    assert_eq!(
        store.require_ready(&source().scope()),
        Err(ArchiveError::IncompleteSource)
    );
    assert_eq!(
        store.finish_import(&session).unwrap().phase,
        ImportPhase::Ready
    );
    assert_eq!(store.require_ready(&source().scope()), Ok(()));
}

#[test]
fn exact_replay_returns_original_checkpoint_without_duplicate_observations() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let input = batch("first", "firstbody");
    let first = store.append_batch(&session, 0, input.clone()).unwrap();
    assert_eq!(first.committed_items, 1);
    assert_eq!(first.next_batch, 1);
    let second = store
        .append_batch(&session, 1, batch("second", "secondbody"))
        .unwrap();
    assert_eq!(second.committed_items, 2);
    assert_eq!(store.append_batch(&session, 0, input).unwrap(), first);
    assert_eq!(stored_texts(&store).len(), 2);
}

#[test]
fn rejected_second_batch_preserves_first_content_and_checkpoint() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let input = batch("first", "firstbody");
    let first = store.append_batch(&session, 0, input.clone()).unwrap();
    let mut invalid = batch("second", "secondbody");
    invalid.contents[0].evidence = EvidenceState::Live;
    assert_eq!(
        store.append_batch(&session, 1, invalid),
        Err(ArchiveError::InvalidRecord)
    );
    assert_eq!(stored_texts(&store), vec!["firstbody"]);
    assert_eq!(store.append_batch(&session, 0, input).unwrap(), first);
}

#[test]
fn receipt_and_counter_failpoints_roll_back_observations_fts_and_findings() {
    for target in ["INSERT ON import_batch_receipts", "UPDATE ON import_runs"] {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        store
            .append_batch(&session, 0, batch("first", "firstbody"))
            .unwrap();
        let before = sql_snapshot(&store);
        let status = checkpoint(&store);
        store.transaction(|tx| { tx.execute_batch(&format!("CREATE TEMP TRIGGER injected_failure BEFORE {target} BEGIN SELECT RAISE(ABORT, 'synthetic failpoint'); END;")).unwrap(); Ok(()) }).unwrap();
        assert_eq!(
            store.append_batch(&session, 1, batch("second", "secondbody test@example.test")),
            Err(ArchiveError::StorageFailure)
        );
        assert_eq!(sql_snapshot(&store), before);
        assert_eq!(checkpoint(&store), status);
        assert_eq!(fts_count(&store, "firstbody"), 1);
        assert_eq!(fts_count(&store, "secondbody"), 0);
        store
            .transaction(|tx| {
                tx.execute_batch("DROP TRIGGER temp.injected_failure")
                    .unwrap();
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .append_batch(&session, 1, batch("second", "secondbody test@example.test"))
                .unwrap()
                .committed_items,
            2
        );
    }
}

#[test]
fn sequence_replay_rejects_changed_data_gaps_and_empty_batches() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let original = batch("one", "samebody");
    assert_eq!(
        store.append_batch(&session, 1, original.clone()),
        Err(ArchiveError::StaleCursor)
    );
    assert_eq!(
        store.append_batch(&session, 0, ImportBatch::default()),
        Err(ArchiveError::InvalidRecord)
    );
    let first = store.append_batch(&session, 0, original.clone()).unwrap();
    assert_eq!(
        store.append_batch(&session, 0, batch("one", "changedbody")),
        Err(ArchiveError::StaleCursor)
    );
    let second = store.append_batch(&session, 1, original.clone()).unwrap();
    assert_eq!(second.committed_items, 1);
    assert_eq!(second.committed_bytes, first.committed_bytes * 2);
    assert_eq!(second.next_batch, 2);
    assert_eq!(store.append_batch(&session, 0, original).unwrap(), first);
    assert_eq!(checkpoint(&store).progress, second);
}

#[test]
fn record_limit_includes_embedded_participants_and_all_explicit_types() {
    for embedded in [false, true] {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        let original = batch("one", "body");
        let mut input = ImportBatch::default();
        if embedded {
            let mut room = original.conversations[0].clone();
            room.participants = vec![original.actors[0].clone(); 499];
            input.conversations.push(room);
        } else {
            input.actors = vec![original.actors[0].clone(); 500];
        }
        let mut over = input.clone();
        over.actors.push(original.actors[0].clone());
        assert_eq!(
            store.append_batch(&session, 0, over),
            Err(ArchiveError::LimitExceeded)
        );
        assert_eq!(
            store
                .append_batch(&session, 0, input)
                .unwrap()
                .committed_items,
            0
        );
    }
}

#[test]
fn encoded_batch_limit_accepts_four_mib_and_rejects_the_next_byte() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = batch("one", "");
    for native in ["two", "three", "four"] {
        input.contents.push(batch(native, "").contents.remove(0));
    }
    let overhead = serde_json::to_vec(&input).unwrap().len();
    for content in input.contents.iter_mut().take(3) {
        content.searchable_text = "a".repeat(1024 * 1024);
    }
    input.contents[3].searchable_text = "b".repeat(1024 * 1024 - overhead);
    assert_eq!(serde_json::to_vec(&input).unwrap().len(), 4 * 1024 * 1024);
    let mut over = input.clone();
    over.contents[3].searchable_text.push('b');
    assert_eq!(
        store.append_batch(&session, 0, over),
        Err(ArchiveError::LimitExceeded)
    );
    assert_eq!(
        store
            .append_batch(&session, 0, input)
            .unwrap()
            .committed_bytes,
        4 * 1024 * 1024
    );
}

#[test]
fn per_item_text_envelope_and_attachment_limits_are_enforced() {
    for (text_bytes, metadata_bytes, attachments, accepted) in [
        (1024 * 1024, 0, 0, true),
        (1024 * 1024 + 1, 0, 0, false),
        (0, 64 * 1024, 0, true),
        (0, 64 * 1024 + 1, 0, false),
        (0, 0, 100, true),
        (0, 0, 101, false),
    ] {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        let mut input = batch("one", &"a".repeat(text_bytes));
        if metadata_bytes > 0 {
            let overhead = serde_json::to_vec(&envelope(String::new())).unwrap().len();
            input.contents[0].provider_metadata =
                Some(envelope("b".repeat(metadata_bytes - overhead)));
        }
        input.contents[0].attachments = vec![attachment("name"); attachments];
        let result = store.append_batch(&session, 0, input);
        if accepted {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert_eq!(result, Err(ArchiveError::LimitExceeded));
        }
    }
}

#[test]
fn cumulative_limits_charge_updates_and_survive_cancel_and_retry() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let input = batch("one", "samebody");
    let bytes = serde_json::to_vec(&input).unwrap().len() as u64;
    store.import_limits = super::model::ImportLimits {
        items: 1,
        bytes: bytes * 2,
    };
    let session = store.begin_import(&source().scope()).unwrap();
    let first = store.append_batch(&session, 0, input.clone()).unwrap();
    assert_eq!(
        store.append_batch(&session, 1, batch("two", "samebody")),
        Err(ArchiveError::LimitExceeded)
    );
    store.cancel_import(&session).unwrap();
    let status = checkpoint(&store);
    let retry = store.retry_import(&status).unwrap();
    assert_eq!(store.append_batch(&retry, 0, input.clone()).unwrap(), first);
    assert_eq!(
        store
            .append_batch(&retry, 1, input.clone())
            .unwrap()
            .committed_bytes,
        bytes * 2
    );
    assert_eq!(
        store.append_batch(&retry, 2, input),
        Err(ArchiveError::LimitExceeded)
    );
    store.cancel_import(&retry).unwrap();
    let status = checkpoint(&store);
    store.import_limits.bytes = bytes;
    assert!(matches!(
        store.retry_import(&status),
        Err(ArchiveError::LimitExceeded)
    ));
}

#[test]
fn nested_actors_cannot_bypass_evidence_scope_metadata_or_duplicate_checks() {
    for mutation in 0..6 {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        let mut input = batch("one", "body");
        let mut actor = input.actors[0].clone();
        match mutation {
            0 => actor.evidence = EvidenceState::LiveAndArchive,
            1 => actor.scope.account_id = Uuid::new_v4().try_into().unwrap(),
            2 => {
                actor.avatar = Some(envelope("unknown".into()));
                actor.avatar.as_mut().unwrap().version = 99;
            }
            3 => actor.display_name = "conflicting duplicate".into(),
            4 => actor.avatar = Some(envelope("large".repeat(64 * 1024))),
            _ => actor.resource.locator_payload = json!({"unrecognized": 1}),
        }
        input.conversations[0].participants.push(actor);
        assert!(store.append_batch(&session, 0, input).is_err());
        assert!(stored_texts(&store).is_empty());
    }
}

#[test]
fn unsupported_adapter_hooks_and_unknown_nested_payload_versions_are_rejected() {
    for target in 0..3 {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        let mut input = batch("one", "body");
        let mut unknown = envelope("unknown".into());
        unknown.version = 2;
        match target {
            0 => input.conversations[0].provider_metadata = Some(unknown),
            1 => input.contents[0].provider_metadata = Some(unknown),
            _ => {
                let mut item = attachment("name");
                item.locator = unknown;
                input.contents[0].attachments.push(item);
            }
        }
        assert_eq!(
            store.append_batch(&session, 0, input),
            Err(ArchiveError::InvalidRecord)
        );
    }
    let fixture = Fixture::new();
    let mut store = ArchiveStore::open(
        fixture.path.clone(),
        super::test_support::key(),
        super::test_support::account_dependent_validators(),
    )
    .unwrap();
    let mut source = source();
    source.schema_profile.payload = json!({"accountName": account().display_name});
    store.register_source(account(), source.clone()).unwrap();
    let session = store.begin_import(&source.scope()).unwrap();
    assert_eq!(
        store.append_batch(&session, 0, batch("one", "body")),
        Err(ArchiveError::InvalidRecord)
    );
}

#[test]
fn findings_come_from_shared_detector_and_attachment_names_have_their_own_index() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = batch("one", "messagebody");
    input.contents[0].privacy_findings = vec![retract_domain::PrivacyKind::CryptoWallet];
    input.contents[0].detector_version = Some("untrusted".into());
    input.contents[0]
        .attachments
        .push(attachment("documentname alice@example.test"));
    input.contents[0].provider_metadata = Some(envelope("metadataonly password".into()));
    store.append_batch(&session, 0, input).unwrap();
    assert_eq!(fts_count(&store, "searchable_text:messagebody"), 1);
    assert_eq!(fts_count(&store, "attachment_names:documentname"), 1);
    assert_eq!(fts_count(&store, "searchable_text:documentname"), 0);
    assert_eq!(fts_count(&store, "metadataonly"), 0);
    let content: ContentRecord = store
        .transaction(|tx| {
            Ok(serde_json::from_str(
                &tx.query_row("SELECT record_json FROM content_observations", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            )
            .unwrap())
        })
        .unwrap();
    assert_eq!(
        content.privacy_findings,
        vec![retract_domain::PrivacyKind::EmailAddress]
    );
    assert!(
        content
            .detector_version
            .unwrap()
            .starts_with("cleaner-sha256:")
    );
    assert_eq!(content.evidence, EvidenceState::Archive);
}

#[test]
fn timestamp_storage_orders_fractional_seconds_and_pre_epoch_values_exactly() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let times = [
        "1970-01-01T00:00:00.000000001Z",
        "1969-12-31T23:59:59.999999999Z",
        "1970-01-01T00:00:00Z",
        "1969-12-31T23:59:59Z",
    ];
    let mut input = batch("one", "body");
    input.contents.clear();
    for (index, timestamp) in times.iter().enumerate() {
        let mut content = batch(&format!("time{index}"), "body").contents.remove(0);
        content.timestamp = timestamp.parse().unwrap();
        input.contents.push(content);
    }
    store.append_batch(&session, 0, input).unwrap();
    let actual = store.transaction(|tx| Ok(tx.prepare("SELECT timestamp_seconds, timestamp_nanos, record_json FROM content_observations ORDER BY timestamp_seconds, timestamp_nanos, resource_id").unwrap().query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, serde_json::from_str::<ContentRecord>(&row.get::<_, String>(2)?).unwrap().timestamp))
    }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap())).unwrap();
    assert_eq!(
        actual.iter().map(|(s, n, _)| (*s, *n)).collect::<Vec<_>>(),
        vec![(-1, 0), (-1, 999_999_999), (0, 0), (0, 1)]
    );
    for ((_, _, value), expected) in actual.iter().zip([times[3], times[1], times[2], times[0]]) {
        assert_eq!(
            *value,
            expected.parse::<chrono::DateTime<chrono::Utc>>().unwrap()
        );
    }
}

#[test]
fn foreign_record_scope_and_conflicting_content_duplicates_leave_no_partial_import() {
    for mutation in 0..4 {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        let mut input = batch("one", "body");
        match mutation {
            0 => input.contents[0].scope.account_id = Uuid::new_v4().try_into().unwrap(),
            1 => input.contents[0].scope.source_id = Uuid::new_v4().try_into().unwrap(),
            2 => input.contents[0].scope.provider = "other".to_owned().try_into().unwrap(),
            _ => {
                let mut conflict = input.contents[0].clone();
                conflict.searchable_text = "conflict".into();
                input.contents.push(conflict);
            }
        }
        let before = sql_snapshot(&store);
        assert!(store.append_batch(&session, 0, input).is_err());
        assert_eq!(sql_snapshot(&store), before);
    }
}

#[test]
fn neutral_leap_second_timestamp_round_trips_between_neighboring_seconds() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = batch("leap", "body");
    input.contents[0].timestamp = "2016-12-31T23:59:60.123456789Z".parse().unwrap();
    store.append_batch(&session, 0, input).unwrap();
    let (seconds, nanos, content) = store.transaction(|tx| Ok(tx.query_row("SELECT timestamp_seconds, timestamp_nanos, record_json FROM content_observations", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))).unwrap())).unwrap();
    assert_eq!((seconds, nanos), (1_483_228_799, 1_123_456_789));
    let record: ContentRecord = serde_json::from_str(&content).unwrap();
    assert_eq!(
        record.timestamp.to_rfc3339(),
        "2016-12-31T23:59:60.123456789+00:00"
    );
    assert!((seconds, nanos) > (1_483_228_799, 999_999_999));
    assert!((seconds, nanos) < (1_483_228_800, 0));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "explicit 100,000-item synthetic ingestion resource gate"]
fn synthetic_hundred_thousand_item_corpus_has_bounded_batch_memory() {
    fn rss_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|value| value.split_whitespace().next())
                    .map(|value| value.parse().unwrap())
            })
            .unwrap()
    }
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let baseline = rss_kib();
    let mut peak = baseline;
    let mut next = 0_u64;
    let mut sequence = 0;
    let template = batch(
        "template",
        "Synthetic corpus message with bounded searchable data.",
    );
    while next < 100_000 {
        let mut input = if sequence == 0 {
            ImportBatch {
                actors: template.actors.clone(),
                conversations: template.conversations.clone(),
                contents: vec![],
            }
        } else {
            ImportBatch::default()
        };
        let count =
            (500 - input.actors.len() - input.conversations.len()).min((100_000 - next) as usize);
        for _ in 0..count {
            let mut content = template.contents[0].clone();
            content.resource = resource(&format!("corpus-{next}"));
            content.id = content.resource.resource_id().unwrap().try_into().unwrap();
            input.contents.push(content);
            next += 1;
        }
        store.append_batch(&session, sequence, input).unwrap();
        sequence += 1;
        peak = peak.max(rss_kib());
    }
    let progress = store.finish_import(&session).unwrap();
    assert_eq!(progress.committed_items, 100_000);
    assert_eq!(progress.next_batch, 201);
    assert!(
        peak.saturating_sub(baseline) < 128 * 1024,
        "RSS grew beyond 128 MiB"
    );
    let disk = std::fs::metadata(&fixture.path).unwrap().len();
    eprintln!(
        "synthetic_corpus items=100000 batches=201 encoded_bytes={} db_bytes={disk} baseline_rss_kib={baseline} peak_sampled_rss_kib={peak}",
        progress.committed_bytes
    );
}
