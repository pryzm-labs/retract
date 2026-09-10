use super::model::{ImportBatchV2, ImportFailureCode, ImportWarningCode, ImportWarningDelta};
use super::{
    ArchiveError, ArchiveService, ArchiveStore,
    model::{ImportDisposition, ImportPhase, NewArchiveImport},
    test_support::{Fixture, account, batch, key, source, validators},
    worker_tests::runtime,
};

fn v2(native: &str, text: &str) -> ImportBatchV2 {
    ImportBatchV2 {
        records: batch(native, text),
        warnings: vec![ImportWarningDelta {
            code: ImportWarningCode::UnknownConversationKind,
            count: 2,
        }],
    }
}

#[test]
fn strict_batch_queue_bound_counts_the_versioned_digest_encoding() {
    let mut input = v2("one", "");
    for native in ["two", "three", "four"] {
        input
            .records
            .contents
            .push(batch(native, "").contents.remove(0));
    }
    let overhead = serde_json::to_vec(&(
        "retract.archive.batch",
        2_u8,
        &input.records,
        &input.warnings,
    ))
    .unwrap()
    .len();
    for content in input.records.contents.iter_mut().take(3) {
        content.searchable_text = "a".repeat(1024 * 1024);
    }
    input.records.contents[3].searchable_text = "b".repeat(1024 * 1024 - overhead);
    assert_eq!(input.bounded_size(), Ok(4 * 1024 * 1024));
    input.records.contents[3].searchable_text.push('b');
    assert_eq!(input.bounded_size(), Err(ArchiveError::LimitExceeded));
}

struct TwoProfiles(std::sync::Arc<dyn crate::persistence::ProviderPayloadValidator>);
impl crate::persistence::ProviderPayloadValidator for TwoProfiles {
    fn validation_policy_key(&self) -> crate::persistence::ProviderValidationPolicyKey {
        "synthetic-two-profiles-v1".to_owned().try_into().unwrap()
    }
    fn validate_account(
        &self,
        a: &retract_domain::AccountRecord,
    ) -> Result<crate::persistence::VerifiedNativeAccountIdentity, crate::error::AppError> {
        self.0.validate_account(a)
    }
    fn validate_source(
        &self,
        s: &retract_domain::SourceRecord,
        a: &retract_domain::AccountRecord,
    ) -> Result<(), crate::error::AppError> {
        let mut legacy = s.clone();
        if legacy.schema_profile.version == 2 {
            legacy.schema_profile.version = 1;
        }
        self.0.validate_source(&legacy, a)
    }
    fn validate_resource(
        &self,
        r: &retract_domain::ProviderResourceRef,
    ) -> Result<(), crate::error::AppError> {
        self.0.validate_resource(r)
    }
    fn validate_recipe(
        &self,
        p: &retract_domain::RemediationPlan,
    ) -> Result<(), crate::error::AppError> {
        self.0.validate_recipe(p)
    }
}

#[test]
fn atomic_registration_keys_the_complete_validated_profile_and_preserves_legacy_accounts() {
    let fixture = Fixture::new();
    let mut registry = validators();
    let validator = registry.remove(&request().provider).unwrap();
    registry.insert(
        request().provider,
        std::sync::Arc::new(TwoProfiles(validator)),
    );
    let store = ArchiveStore::open(fixture.path.clone(), key(), registry.clone()).unwrap();
    store.register_source(account(), source()).unwrap();
    let first = store.resolve_or_register_import(request()).unwrap();
    assert_eq!(first.account.id, account().id);
    assert_ne!(first.source.id, source().id);
    let mut different = request();
    different.schema_profile.version = 2;
    let other = store.resolve_or_register_import(different.clone()).unwrap();
    assert_eq!(other.account, first.account);
    assert_ne!(other.source.id, first.source.id);
    assert_eq!(
        store
            .resolve_or_register_import(different.clone())
            .unwrap()
            .source,
        other.source
    );
    drop(store);
    let store = ArchiveStore::open(fixture.path.clone(), key(), registry).unwrap();
    assert_eq!(
        store
            .resolve_or_register_import(different)
            .unwrap()
            .source
            .id,
        other.source.id
    );
}

#[test]
fn strict_batches_ignore_derived_fields_but_reject_provider_conflicts_within_and_across_batches() {
    let fixture = Fixture::new();
    let mut store = super::test_support::registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = v2("one", "owner@example.test");
    let mut derived = input.records.contents[0].clone();
    derived.privacy_findings = vec![retract_domain::PrivacyKind::CryptoWallet];
    derived.detector_version = Some("outdated".into());
    input.records.contents.push(derived);
    let first = store.append_batch_v2(&session, 0, input.clone()).unwrap();
    assert_eq!(first.committed_items, 1);
    let before = super::test_support::sql_snapshot(&store);
    input.records.contents[1].detector_version = None;
    assert_eq!(
        store.append_batch_v2(&session, 0, input.clone()).unwrap(),
        first
    );
    assert_eq!(super::test_support::sql_snapshot(&store), before);
    let second = store
        .append_batch_v2(&session, 1, v2("one", "owner@example.test"))
        .unwrap();
    assert_eq!(second.committed_items, 1);
    for variant in 0..5 {
        let mut conflict = v2("one", "owner@example.test");
        match variant {
            0 => conflict.records.contents[0].searchable_text = "changed".into(),
            1 => conflict.records.actors[0].display_name = "changed".into(),
            2 => conflict.records.conversations[0].title = "changed".into(),
            3 => {
                let mut other = conflict.records.contents[0].clone();
                other.searchable_text = "changed".into();
                conflict.records.contents.push(other);
            }
            _ => conflict.records.contents[0].observed_at += chrono::Duration::seconds(1),
        }
        let before = super::test_support::sql_snapshot(&store);
        assert!(store.append_batch_v2(&session, 2, conflict).is_err());
        assert_eq!(super::test_support::sql_snapshot(&store), before);
    }
    let legacy = store
        .append_batch(&session, 2, batch("one", "legacy update"))
        .unwrap();
    assert_eq!(legacy.committed_items, 1);
    assert_eq!(super::test_support::stored_texts(&store), ["legacy update"]);
}

#[test]
fn warning_receipts_preserve_order_replay_quotas_and_survive_restart_and_finalization() {
    let fixture = Fixture::new();
    let mut store = super::test_support::registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    let mut input = v2("one", "body");
    input.records.contents[0].reply_to = Some(uuid::Uuid::from_u128(99).try_into().unwrap());
    input.warnings.push(ImportWarningDelta {
        code: ImportWarningCode::MissingOptionalContext,
        count: 3,
    });
    let progress = store.append_batch_v2(&session, 0, input.clone()).unwrap();
    let checkpoint = super::test_support::checkpoint(&store);
    assert_eq!(
        checkpoint
            .warnings
            .iter()
            .map(|w| (w.code.as_str(), w.count))
            .collect::<Vec<_>>(),
        [
            ("missing_optional_context", 3),
            ("unknown_conversation_kind", 2)
        ]
    );
    let mut reversed = input.clone();
    reversed.warnings.reverse();
    assert_eq!(
        store.append_batch_v2(&session, 0, reversed),
        Err(ArchiveError::StaleCursor)
    );
    drop(store);
    let mut store = fixture.open();
    let interrupted = super::test_support::checkpoint(&store);
    assert_eq!(interrupted.observed_at, checkpoint.observed_at);
    let retry = store.retry_import(&interrupted).unwrap();
    assert_eq!(store.append_batch_v2(&retry, 0, input).unwrap(), progress);
    assert_eq!(
        super::test_support::checkpoint(&store).warnings,
        checkpoint.warnings
    );
    let warning_only = ImportBatchV2 {
        records: Default::default(),
        warnings: vec![ImportWarningDelta {
            code: ImportWarningCode::MissingOptionalContext,
            count: 1,
        }],
    };
    store.append_batch_v2(&retry, 1, warning_only).unwrap();
    store.finish_import(&retry).unwrap();
    let ready = super::test_support::checkpoint(&store);
    assert_eq!(
        ready
            .warnings
            .iter()
            .map(|w| (w.code.as_str(), w.count))
            .collect::<Vec<_>>(),
        [
            ("missing_optional_context", 4),
            ("missing_reply", 1),
            ("unknown_conversation_kind", 2)
        ]
    );
    assert_eq!(store.cancel_import(&retry), Err(ArchiveError::StaleCursor));
    assert_eq!(
        store.fail_import(&retry, ImportFailureCode::InvalidArchive),
        Err(ArchiveError::StaleCursor)
    );
    assert_eq!(super::test_support::checkpoint(&store), ready);
    drop(store);
    assert_eq!(super::test_support::checkpoint(&fixture.open()), ready);
}

#[test]
fn warning_codes_counts_and_encoded_batches_fail_closed_without_writes() {
    let fixture = Fixture::new();
    let mut store = super::test_support::registered(&fixture);
    let session = store.begin_import(&source().scope()).unwrap();
    for (count, entries) in [(0, 1), (u64::MAX, 1), (1, 33)] {
        let mut input = v2("one", "body");
        input.warnings = vec![
            ImportWarningDelta {
                code: ImportWarningCode::UnknownConversationKind,
                count
            };
            entries
        ];
        assert!(store.append_batch_v2(&session, 0, input).is_err());
    }
    assert!(serde_json::from_str::<ImportWarningCode>("\"owner@example.test\"").is_err());
    assert!(serde_json::from_str::<ImportFailureCode>("\"/private/archive.zip\"").is_err());
    assert!(super::test_support::stored_texts(&store).is_empty());
    let mut maximal = v2("one", "body");
    maximal.warnings[0].count = i64::MAX as u64;
    store.append_batch_v2(&session, 0, maximal).unwrap();
    let before = super::test_support::sql_snapshot(&store);
    assert_eq!(
        store.append_batch_v2(&session, 1, v2("two", "other")),
        Err(ArchiveError::LimitExceeded)
    );
    assert_eq!(super::test_support::sql_snapshot(&store), before);
}

#[test]
fn failure_requires_live_session_preserves_progress_and_keeps_partial_source_hidden() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let service = ArchiveService::open(move || ArchiveStore::open(path, key(), validators()))
            .await
            .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        let progress = service
            .append_batch_v2(&session, 0, &v2("one", "hiddenneedle"))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let failed = service
            .fail_import(&session, ImportFailureCode::InputChanged)
            .await
            .unwrap();
        assert_eq!(failed.committed_items, progress.committed_items);
        assert_eq!(failed.committed_bytes, progress.committed_bytes);
        assert_eq!(failed.next_batch, progress.next_batch);
        assert_eq!(failed.phase, ImportPhase::Failed);
        assert_eq!(
            service
                .search(&super::worker_tests::search(
                    source().scope(),
                    "hiddenneedle"
                ))
                .await
                .err(),
            Some(ArchiveError::IncompleteSource)
        );
        let status = service
            .import_status(
                &source().scope(),
                &request().fingerprint,
                &request().schema_profile,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.failure_code, Some(ImportFailureCode::InputChanged));
        let retry = service.retry_import(&status).await.unwrap();
        assert_eq!(
            service
                .fail_import(&session, ImportFailureCode::InvalidArchive)
                .await,
            Err(ArchiveError::StaleCursor)
        );
        service.finish_import(&retry).await.unwrap();
        assert_eq!(
            service.cancel_import(&retry).await,
            Err(ArchiveError::StaleCursor)
        );
        let ready = service
            .import_status(
                &source().scope(),
                &request().fingerprint,
                &request().schema_profile,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.failure_code, None);
        assert_eq!(ready.observed_at, status.observed_at);
        service.shutdown().await;
    });
}

fn request() -> NewArchiveImport {
    let owner = account();
    NewArchiveImport {
        provider: owner.provider,
        native_identity: owner.native_identity,
        display_name: owner.display_name,
        username: owner.username,
        avatar: owner.avatar,
        fingerprint: source().archive_fingerprint.unwrap(),
        schema_profile: source().schema_profile,
        parser_policy: "synthetic.import.v1".into(),
        observed_at: source().updated_at,
    }
}

#[test]
fn reopening_rejects_corrupt_identity_and_warning_ledgers_before_any_run_mutation() {
    for mutation in [
        "UPDATE archive_import_identities SET fingerprint='wrong'",
        "UPDATE archive_import_identities SET parser_policy=''",
        "UPDATE import_warnings SET count=count+1",
        "UPDATE import_warning_deltas SET ordinal=ordinal+1",
        "UPDATE import_batch_receipts SET digest_version=1",
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let mut registered = request();
        registered.native_identity.payload["nativeId"] = serde_json::json!("123");
        store.resolve_or_register_import(registered).unwrap();
        store.register_source(account(), source()).unwrap();
        let mut store = store;
        let session = store.begin_import(&source().scope()).unwrap();
        store
            .append_batch_v2(&session, 0, v2("one", "body"))
            .unwrap();
        drop(store);
        let db = super::codec::open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        let before = super::test_support::snapshot_recovery_files(&fixture.path);
        assert!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()).is_err(),
            "{mutation}"
        );
        assert_eq!(
            super::test_support::snapshot_recovery_files(&fixture.path),
            before
        );
    }
}

#[test]
fn atomic_registration_race_returns_one_writer_and_complete_persisted_identity() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let service = ArchiveService::open(move || ArchiveStore::open(path, key(), validators()))
            .await
            .unwrap();
        let (one, two) = futures_util::future::join(
            service.resolve_or_register_import(request()),
            service.resolve_or_register_import(request()),
        )
        .await;
        let one = one.unwrap();
        let two = two.unwrap();
        assert_eq!(
            (one.disposition, two.disposition),
            (ImportDisposition::Start, ImportDisposition::Busy)
        );
        assert!(one.session.is_some());
        assert!(two.session.is_none());
        assert_eq!(one.account, two.account);
        assert_eq!(one.source, two.source);
        assert_eq!(one.checkpoint, two.checkpoint);
        assert!(!one.account.id.as_uuid().is_nil());
        assert!(!one.source.id.as_uuid().is_nil());
        assert_eq!(one.account.id, one.source.account_id);
        assert_eq!(one.checkpoint.observed_at, request().observed_at);
        let session = one.session.unwrap();
        service.finish_import(&session).await.unwrap();
        let ready = service.resolve_or_register_import(request()).await.unwrap();
        assert_eq!(ready.disposition, ImportDisposition::Ready);
        assert!(ready.session.is_none());
        service.shutdown().await;
        let store = ArchiveStore::open(fixture.path.clone(), key(), validators()).unwrap();
        let again = store.resolve_or_register_import(request()).unwrap();
        assert_eq!(again.disposition, ImportDisposition::Ready);
        assert_eq!(again.source, ready.source);
        assert_eq!(again.checkpoint, ready.checkpoint);
    });
}

#[test]
fn atomic_registration_distinguishes_snapshots_policies_accounts_and_rejects_invalid_payloads() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let first = store.resolve_or_register_import(request()).unwrap();
    let mut equivalent = request();
    equivalent.native_identity.payload =
        serde_json::json!({"nativeId": "20000000000000", "encoding": "hex"});
    equivalent.display_name = "Different display observation".into();
    equivalent.observed_at += chrono::Duration::days(10);
    let same = store.resolve_or_register_import(equivalent).unwrap();
    assert_eq!(same.source, first.source);
    assert_eq!(same.account, first.account);
    assert_eq!(same.checkpoint.observed_at, first.checkpoint.observed_at);
    let mut snapshot = request();
    snapshot.fingerprint.push('2');
    let newer = store.resolve_or_register_import(snapshot).unwrap();
    assert_eq!(newer.account, first.account);
    assert_ne!(newer.source.id, first.source.id);
    let mut policy = request();
    policy.parser_policy = "synthetic.import.v2".into();
    let policy = store.resolve_or_register_import(policy).unwrap();
    assert_ne!(policy.source.id, first.source.id);
    assert_eq!(policy.account, first.account);
    let mut different = request();
    different.native_identity.payload["nativeId"] = serde_json::json!("9007199254740993");
    let different = store.resolve_or_register_import(different).unwrap();
    assert_ne!(different.account.id, first.account.id);
    assert_ne!(different.source.id, first.source.id);
    assert_eq!(different.account.display_name, first.account.display_name);
    for mutate in 0..4 {
        let mut invalid = request();
        match mutate {
            0 => invalid.native_identity.version = 99,
            1 => invalid.provider = "foreign".to_owned().try_into().unwrap(),
            2 => invalid.schema_profile.version = 99,
            _ => invalid.parser_policy = "untrusted\npolicy".into(),
        }
        assert!(store.resolve_or_register_import(invalid).is_err());
    }
    let mut wrong = first.source.scope();
    wrong.account_id = different.account.id;
    assert_eq!(store.source(&wrong), Err(ArchiveError::ScopeMismatch));
    store.transaction(|tx| {
        assert_eq!(tx.query_row("SELECT count(*) FROM accounts", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(tx.query_row("SELECT count(*) FROM archive_import_identities", [], |r| r.get::<_, i64>(0)).unwrap(), 4);
        let plan: Vec<String> = tx.prepare("EXPLAIN QUERY PLAN SELECT source_id FROM archive_import_identities WHERE provider=?1 AND account_id=?2 AND fingerprint=?3 AND schema_profile=?4 AND parser_policy=?5 AND validation_policy=?6").unwrap().query_map([""; 6], |r| r.get(3)).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(plan.iter().any(|p| p.contains("SEARCH archive_import_identities USING INDEX")));
        Ok(())
    }).unwrap();
}

#[test]
fn atomic_registration_interruption_cancellation_failure_and_removal_never_start_implicitly() {
    for phase in [
        ImportPhase::Interrupted,
        ImportPhase::Cancelled,
        ImportPhase::Failed,
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let first = store.resolve_or_register_import(request()).unwrap();
        let session = first.session.unwrap();
        if phase == ImportPhase::Cancelled {
            store.cancel_import(&session).unwrap();
        }
        if phase == ImportPhase::Failed {
            let mut input = batch("orphan", "hidden");
            let content = &mut input.contents[0];
            content.scope = first.source.scope();
            content.resource.account_id = first.account.id;
            content.id = content.resource.resource_id().unwrap().try_into().unwrap();
            input.actors.clear();
            input.conversations.clear();
            store.append_batch(&session, 0, input).unwrap();
            assert_eq!(
                store.finish_import(&session),
                Err(ArchiveError::IncompleteSource)
            );
        }
        drop(store);
        let mut store = fixture.open();
        let retry = store.resolve_or_register_import(request()).unwrap();
        assert_eq!(retry.disposition, ImportDisposition::RetryRequired);
        assert_eq!(retry.checkpoint.progress.phase, phase);
        assert_eq!(retry.checkpoint.observed_at, first.checkpoint.observed_at);
        assert!(retry.session.is_none());
        assert_eq!(retry.source.id, first.source.id);
        assert_eq!(
            store.require_ready(&retry.source.scope()),
            Err(ArchiveError::IncompleteSource)
        );
        store.remove_source(&retry.source.scope()).unwrap();
        let fresh = store.resolve_or_register_import(request()).unwrap();
        assert_eq!(fresh.disposition, ImportDisposition::Start);
        assert_ne!(fresh.source.id, first.source.id);
        assert!(store.cancel_import(&session).is_err());
    }
}
