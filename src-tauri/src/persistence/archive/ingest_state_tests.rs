use uuid::Uuid;

use super::{
    ArchiveError, ArchiveStore,
    model::{ImportBatch, ImportPhase},
    test_support::{
        Fixture, account, batch, checkpoint, fts_count, registered, source, sql_snapshot,
        stored_texts,
    },
};

#[test]
fn restart_exposes_interrupted_checkpoint_and_retry_rotates_mutation_authority() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let original = batch("one", "firstbody");
    let first = store.append_batch(&session, 0, original.clone()).unwrap();
    store
        .append_batch(&session, 1, batch("two", "secondbody"))
        .unwrap();
    let before = checkpoint(&store);
    drop(store);
    let mut store = fixture.open();
    let interrupted = checkpoint(&store);
    assert_eq!(interrupted.progress.phase, ImportPhase::Interrupted);
    assert_eq!(interrupted.run_id, before.run_id);
    assert_eq!(
        interrupted.progress.committed_bytes,
        before.progress.committed_bytes
    );
    assert_eq!(interrupted.progress.next_batch, 2);
    assert!(store.retry_import(&before).is_err());
    assert!(
        store
            .append_batch(&session, 2, batch("three", "thirdbody"))
            .is_err()
    );
    let retry = store.retry_import(&interrupted).unwrap();
    assert_ne!(retry.id, session.id);
    assert_eq!(checkpoint(&store).run_id, before.run_id);
    assert!(store.retry_import(&interrupted).is_err());
    assert!(
        store
            .append_batch(&session, 2, batch("three", "thirdbody"))
            .is_err()
    );
    assert_eq!(store.append_batch(&retry, 0, original).unwrap(), first);
    assert_eq!(
        store
            .append_batch(&retry, 2, batch("three", "thirdbody"))
            .unwrap()
            .committed_items,
        3
    );
}

#[test]
fn cancellation_and_provenance_require_an_explicit_current_checkpoint() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let mut session = store.begin_import(&source().scope()).unwrap();
    session.fingerprint.push('x');
    assert!(store.finish_import(&session).is_err());
    assert!(store.cancel_import(&session).is_err());
    assert!(
        store
            .append_batch(&session, 0, batch("one", "body"))
            .is_err()
    );
    session.fingerprint.pop();
    assert_eq!(
        store.cancel_import(&session).unwrap().phase,
        ImportPhase::Cancelled
    );
    assert_eq!(
        store.append_batch(&session, 0, batch("one", "body")),
        Err(ArchiveError::Cancelled)
    );
    let current = checkpoint(&store);
    assert!(
        store
            .import_status(
                &source().scope(),
                "wrong fingerprint",
                &source().schema_profile
            )
            .is_err()
    );
    for mutation in 0..4 {
        let mut wrong = current.clone();
        match mutation {
            0 => wrong.fingerprint.push('x'),
            1 => wrong.schema_profile.version += 1,
            2 => wrong.run_id = Uuid::new_v4(),
            _ => wrong.revision += 1,
        }
        assert!(store.retry_import(&wrong).is_err());
    }
    drop(store);
    let mut store = fixture.open();
    assert_eq!(checkpoint(&store), current);
    let session = store.retry_import(&current).unwrap();
    assert_eq!(store.finish_import(&session).unwrap().committed_items, 0);
}

#[test]
fn finalization_requires_same_source_mandatory_observations_but_keeps_optional_ids() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = batch("one", "body");
    let references = ImportBatch {
        actors: input.actors.clone(),
        conversations: input.conversations.clone(),
        contents: vec![],
    };
    input.actors.clear();
    input.conversations.clear();
    input.contents[0].reply_to = Some(Uuid::new_v4().try_into().unwrap());
    input.contents[0].thread_parent = Some(Uuid::new_v4().try_into().unwrap());
    store.append_batch(&session, 0, input).unwrap();
    assert_eq!(
        store.finish_import(&session),
        Err(ArchiveError::IncompleteSource)
    );
    assert_eq!(checkpoint(&store).progress.phase, ImportPhase::Failed);
    let status = checkpoint(&store);
    let retry = store.retry_import(&status).unwrap();
    store.append_batch(&retry, 1, references).unwrap();
    assert_eq!(
        store.finish_import(&retry).unwrap().phase,
        ImportPhase::Ready
    );
    let warnings = checkpoint(&store).warnings;
    assert_eq!(
        warnings
            .iter()
            .map(|w| (w.code.as_str(), w.count))
            .collect::<Vec<_>>(),
        vec![("missing_reply", 1), ("missing_thread", 1)]
    );
}

#[test]
fn cancellation_signal_before_work_and_before_commit_prevents_all_batch_writes() {
    for during_commit in [false, true] {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        store
            .append_batch(&session, 0, batch("one", "firstbody"))
            .unwrap();
        let before = sql_snapshot(&store);
        let signal = session.cancellation_signal();
        if during_commit {
            store.before_commit = Some(Box::new(move || signal.cancel()));
        } else {
            signal.cancel();
        }
        assert_eq!(
            store.append_batch(&session, 1, batch("two", "secondbody test@example.test")),
            Err(ArchiveError::Cancelled)
        );
        store.before_commit = None;
        assert_eq!(sql_snapshot(&store), before);
        assert_eq!(fts_count(&store, "secondbody"), 0);
        store.cancel_import(&session).unwrap();
        let status = checkpoint(&store);
        let retry = store.retry_import(&status).unwrap();
        assert_eq!(
            store
                .append_batch(&retry, 1, batch("two", "secondbody"))
                .unwrap()
                .committed_items,
            2
        );
    }
}

#[test]
fn independent_source_observations_keep_shared_ids_and_cascade_only_their_own_receipts() {
    let fixture = Fixture::new();
    let mut store = registered(&fixture);
    let first = store.begin_import(&source().scope()).unwrap();
    store
        .append_batch(&first, 0, batch("one", "earlierbody"))
        .unwrap();
    store.finish_import(&first).unwrap();
    let mut other = source();
    other.id = Uuid::new_v4().try_into().unwrap();
    other.archive_fingerprint = Some("synthetic-export-02".into());
    store.register_source(account(), other.clone()).unwrap();
    let second = store.begin_import(&other.scope()).unwrap();
    let mut input = batch("one", "laterbody");
    for actor in &mut input.actors {
        actor.scope = other.scope();
    }
    for conversation in &mut input.conversations {
        conversation.scope = other.scope();
    }
    for content in &mut input.contents {
        content.scope = other.scope();
    }
    let relationships = ImportBatch {
        actors: input.actors.clone(),
        conversations: input.conversations.clone(),
        contents: vec![],
    };
    input.actors.clear();
    input.conversations.clear();
    store.append_batch(&second, 0, input).unwrap();
    assert_eq!(
        store.finish_import(&second),
        Err(ArchiveError::IncompleteSource)
    );
    let status = store
        .import_status(
            &other.scope(),
            other.archive_fingerprint.as_deref().unwrap(),
            &other.schema_profile,
        )
        .unwrap()
        .unwrap();
    let retry = store.retry_import(&status).unwrap();
    store.append_batch(&retry, 1, relationships).unwrap();
    store.finish_import(&retry).unwrap();
    assert_eq!(stored_texts(&store), vec!["earlierbody", "laterbody"]);
    store
        .transaction(|tx| {
            assert_eq!(
                tx.query_row("SELECT count(*) FROM resource_identities", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                3
            );
            tx.execute(
                "DELETE FROM sources WHERE source_id=?",
                [other.id.as_uuid().to_string()],
            )
            .unwrap();
            assert_eq!(
                tx.query_row("SELECT count(*) FROM import_batch_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                1
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(stored_texts(&store), vec!["earlierbody"]);
    assert_eq!(fts_count(&store, "laterbody"), 0);
    assert_eq!(fts_count(&store, "earlierbody"), 1);
}

#[test]
fn reopen_rejects_corrupt_progress_and_registration_before_interrupting_any_run() {
    for mutation in [
        "UPDATE import_runs SET committed_bytes=committed_bytes+1",
        "UPDATE sources SET record_json=json_set(record_json, '$.schemaProfile.version', 99)",
    ] {
        let fixture = Fixture::new();
        let mut store = registered(&fixture);
        let session = store.begin_import(&source().scope()).unwrap();
        store
            .append_batch(&session, 0, batch("one", "body"))
            .unwrap();
        drop(store);
        let connection =
            super::codec::open_keyed(&fixture.path, &super::test_support::key(), false).unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        let before = super::test_support::snapshot_recovery_files(&fixture.path);
        assert!(
            ArchiveStore::open(
                fixture.path.clone(),
                super::test_support::key(),
                super::test_support::validators()
            )
            .is_err()
        );
        assert_eq!(
            super::test_support::snapshot_recovery_files(&fixture.path),
            before
        );
        let connection =
            super::codec::open_keyed(&fixture.path, &super::test_support::key(), false).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT state FROM import_runs", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "importing"
        );
    }
}
