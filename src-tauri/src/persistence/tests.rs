use std::{
    fs, io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use cleaner_domain::PlanOperation;
use retract_domain::{
    AccountRecord, ActionDescriptor, ActionStep, Availability, BatchConstraints,
    ConfirmationRequirements, ConfirmationTier, ErrorCode, ExpectedEffect, JobCounters, JobStatus,
    LegacyOperation, LegacyTerminalStatus, ProviderKey, ProviderResourceRef, RemediationPlan,
    ResourceKind, RestartPolicy, Scope, ScopedJobRecord, ScopedResourceRef, SourceRecord,
    VersionedPayload,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FoundationState, FoundationStore, LegacyStoreFormat, ProviderPayloadValidator, RealStoreIo,
    StoreBinding, StoreIo, VerifiedNativeAccountIdentity, foundation_store::store_aad,
};
use crate::{
    error::AppError,
    model::{JobStatus as LegacyJobStatus, PersistedState},
    secure_store::{SecureJobStore, decrypt_authenticated},
};

const LEGACY_KEY: [u8; 32] = [0x61; 32];
const CURRENT_KEY: [u8; 32] = [0x62; 32];
const PROFILE: &str = "telegram-compatibility";

fn provider(value: &str) -> ProviderKey {
    serde_json::from_value(json!(value)).unwrap()
}

fn binding() -> StoreBinding {
    StoreBinding {
        provider: provider("telegram"),
        profile: PROFILE.into(),
    }
}

#[derive(Debug)]
struct StrictTestPayloadValidator;

impl ProviderPayloadValidator for StrictTestPayloadValidator {
    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct NativeIdentity {
            environment: String,
            user_id: String,
        }

        if account.native_identity.schema != "telegram.account"
            || account.native_identity.version != 1
            || account.avatar.is_some()
        {
            return Err(invalid_test_payload());
        }
        let identity: NativeIdentity =
            serde_json::from_value(account.native_identity.payload.clone())
                .map_err(|_| invalid_test_payload())?;
        if identity.environment != "test" || !canonical_positive_i64(&identity.user_id) {
            return Err(invalid_test_payload());
        }
        VerifiedNativeAccountIdentity::try_from(format!(
            "{}:{}",
            identity.environment, identity.user_id
        ))
    }

    fn validate_source(
        &self,
        source: &SourceRecord,
        _account: &AccountRecord,
    ) -> Result<(), AppError> {
        if source.schema_profile.schema != "telegram.live"
            || source.schema_profile.version != 1
            || source.schema_profile.payload != json!({})
        {
            return Err(invalid_test_payload());
        }
        Ok(())
    }

    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct MessageLocator {
            chat_id: String,
            message_id: String,
        }

        if resource.provider != provider("telegram")
            || resource.resource_kind != ResourceKind::Content
            || resource.locator_schema != "telegram.message"
            || resource.locator_version != 1
        {
            return Err(invalid_test_payload());
        }
        let locator: MessageLocator = serde_json::from_value(resource.locator_payload.clone())
            .map_err(|_| invalid_test_payload())?;
        if !canonical_nonzero_i64(&locator.chat_id)
            || !canonical_positive_i64(&locator.message_id)
            || resource.canonical_key
                != serde_json::to_string(&(locator.chat_id, locator.message_id)).unwrap()
        {
            return Err(invalid_test_payload());
        }
        Ok(())
    }

    fn validate_recipe(&self, plan: &RemediationPlan) -> Result<(), AppError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Recipe {
            operation: String,
            batch_size: u32,
            target_keys: Vec<String>,
        }

        if plan.recipe.schema != "telegram.delete-messages" || plan.recipe.version != 1 {
            return Err(invalid_test_payload());
        }
        let recipe: Recipe = serde_json::from_value(plan.recipe.payload.clone())
            .map_err(|_| invalid_test_payload())?;
        let target_keys = plan
            .targets
            .iter()
            .map(|target| target.resource.canonical_key.clone())
            .collect::<Vec<_>>();
        if recipe.operation != "selected"
            || recipe.batch_size != 1
            || recipe.target_keys != target_keys
            || plan
                .steps
                .iter()
                .any(|step| step.descriptor.batch.max_targets != recipe.batch_size)
        {
            return Err(invalid_test_payload());
        }
        Ok(())
    }
}

fn invalid_test_payload() -> AppError {
    AppError::SecureStore("synthetic provider payload validation failed".into())
}

fn canonical_nonzero_i64(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|parsed| parsed != 0 && parsed.to_string() == value)
}

fn canonical_positive_i64(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|parsed| parsed > 0 && parsed.to_string() == value)
}

fn strict_validator() -> Arc<dyn ProviderPayloadValidator> {
    Arc::new(StrictTestPayloadValidator)
}

fn open_validated(directory: &Path) -> Arc<FoundationStore> {
    FoundationStore::open_with_test_key_and_payload_validator(
        directory.to_path_buf(),
        binding(),
        CURRENT_KEY,
        strict_validator(),
    )
    .unwrap()
}

fn fixture(name: &str) -> Vec<u8> {
    let encoded = match name {
        "rtrct01" => include_str!("../../tests/fixtures/secure-store/rtrct01-nonempty.b64"),
        "rtrct02" => include_str!("../../tests/fixtures/secure-store/rtrct02-nonempty.b64"),
        _ => panic!("unknown fixture"),
    };
    BASE64.decode(encoded.trim()).unwrap()
}

fn write_fixture(directory: &Path, name: &str) -> Vec<u8> {
    let bytes = fixture(name);
    fs::write(directory.join("jobs.enc"), &bytes).unwrap();
    bytes
}

fn lifecycle_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../src/test/fixtures/provider-lifecycle.json"
    ))
    .unwrap()
}

fn scope() -> Scope {
    serde_json::from_value(lifecycle_fixture()["context"]["scope"].clone()).unwrap()
}

fn target(index: usize) -> ScopedResourceRef {
    serde_json::from_value(lifecycle_fixture()["messages"][index]["ref"].clone()).unwrap()
}

fn account() -> AccountRecord {
    serde_json::from_value(json!({
        "id": scope().account_id,
        "provider": "telegram",
        "nativeIdentity": {
            "schema": "telegram.account",
            "version": 1,
            "payload": {"environment": "test", "userId": "42"}
        },
        "displayName": "Synthetic account",
        "username": null,
        "avatar": null,
        "connectionState": "ready",
        "createdAt": "2026-09-03T00:00:00Z",
        "lastSeenAt": "2026-09-03T00:00:01Z"
    }))
    .unwrap()
}

fn source() -> SourceRecord {
    serde_json::from_value(json!({
        "id": scope().source_id,
        "accountId": scope().account_id,
        "provider": "telegram",
        "kind": "live_connection",
        "state": "ready",
        "archiveFingerprint": null,
        "schemaProfile": {"schema": "telegram.live", "version": 1, "payload": {}},
        "importedAt": null,
        "updatedAt": "2026-09-03T00:00:01Z",
        "warnings": []
    }))
    .unwrap()
}

fn plan() -> RemediationPlan {
    let targets = vec![target(0), target(1)];
    let target_keys = targets
        .iter()
        .map(|target| target.resource.canonical_key.clone())
        .collect::<Vec<_>>();
    let mut plan = RemediationPlan {
        id: Uuid::parse_str("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap(),
        scope: scope(),
        steps: vec![ActionStep {
            descriptor: ActionDescriptor {
                id: "delete-selected".into(),
                kind: retract_domain::ActionKind::DeleteRemoteItem,
                effect: ExpectedEffect::RemovedForAllParticipants,
                availability: Availability::Executable,
                unavailable_reason: None,
                requires_live_preflight: true,
                batch: BatchConstraints {
                    max_targets: 1,
                    max_parallel: 1,
                },
                confirmation_tier: ConfirmationTier::Low,
                destructive: true,
                irreversible: true,
                advisory: None,
            },
            targets: targets.clone(),
        }],
        targets,
        confirmation: ConfirmationRequirements {
            tier: ConfirmationTier::Low,
            acknowledgement_required: true,
            owner_auth_required: true,
            exact_text: None,
        },
        recipe: VersionedPayload {
            schema: "telegram.delete-messages".into(),
            version: 1,
            payload: json!({
                "operation": "selected",
                "batchSize": 1,
                "targetKeys": target_keys
            }),
        },
        restart_policy: RestartPolicy::ResumeFrozenTargets,
        created_at: "2026-09-03T00:00:00Z".parse().unwrap(),
        fingerprint: String::new(),
    };
    plan.seal().unwrap();
    plan
}

fn job(plan: &RemediationPlan) -> ScopedJobRecord {
    ScopedJobRecord {
        id: Uuid::parse_str("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee").unwrap(),
        plan_id: plan.id,
        scope: plan.scope.clone(),
        dirty_refs: plan.targets.clone(),
        status: JobStatus::Queued,
        counters: JobCounters {
            selected: 2,
            eligible: 2,
            ..JobCounters::default()
        },
        next_batch: 0,
        retry_at: None,
        diagnostics: Vec::new(),
        started_authorized: false,
        created_at: "2026-09-03T00:00:01Z".parse().unwrap(),
        updated_at: "2026-09-03T00:00:01Z".parse().unwrap(),
    }
}

fn install_valid_graph(state: &mut FoundationState) {
    let plan = plan();
    let job = job(&plan);
    state.identities.push(account());
    state.sources.push(source());
    state.plans.push(plan);
    state.jobs.push(job);
}

#[test]
fn migrates_frozen_rtrct01_with_an_exact_backup_and_stopped_unbound_history() {
    let directory = tempfile::tempdir().unwrap();
    let original = write_fixture(directory.path(), "rtrct01");

    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), LEGACY_KEY)
            .unwrap();
    let state = store.snapshot().unwrap();

    assert_eq!(
        fs::read(directory.path().join("jobs.pre-provider.enc")).unwrap(),
        original
    );
    assert!(
        fs::read(directory.path().join("jobs.enc"))
            .unwrap()
            .starts_with(b"RTRCT03")
    );
    assert!(state.identities.is_empty());
    assert!(state.sources.is_empty());
    assert!(state.plans.is_empty());
    assert!(state.jobs.is_empty());
    assert_eq!(state.legacy_history.len(), 2);
    assert!(state.legacy_history.iter().all(|row| !row.executable));
    assert_eq!(
        state.legacy_history[0].record.status,
        LegacyTerminalStatus::Completed
    );
    assert_eq!(
        state.legacy_history[1].record.status,
        LegacyTerminalStatus::Failed
    );
    assert!(
        state.legacy_history[1]
            .record
            .diagnostics
            .iter()
            .any(|error| {
                error.code == ErrorCode::MigrationRequiresNewReview && error.retry_at.is_none()
            })
    );
    assert_eq!(
        state.migration.as_ref().unwrap().source_format,
        LegacyStoreFormat::Rtrct01
    );
    assert_eq!(
        state.migration.as_ref().unwrap().source_sha256,
        "sha256:cdf177c6946e7ad69b9cd4869876c5b6256e45a952acb1137d8dc6efac34ed9e"
    );
    assert_eq!(
        state.migration.as_ref().unwrap().source_bytes,
        original.len() as u64
    );
}

#[test]
fn migrates_frozen_rtrct02_and_keeps_the_backup_readable_by_the_old_reader() {
    let directory = tempfile::tempdir().unwrap();
    let original = write_fixture(directory.path(), "rtrct02");

    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let state = store.snapshot().unwrap();
    let backup = directory.path().join("jobs.pre-provider.enc");

    assert_eq!(fs::read(&backup).unwrap(), original);
    assert_eq!(
        state.migration.as_ref().unwrap().source_format,
        LegacyStoreFormat::Rtrct02
    );
    assert_eq!(
        state.migration.as_ref().unwrap().source_sha256,
        "sha256:80ad7efbf023b920ff3f52c8bd22a7ae6d86d0d93199a1c966156eebc1684309"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        SecureJobStore::with_test_key_and_profile(backup, CURRENT_KEY, PROFILE.as_bytes())
            .load()
            .is_ok()
    );
    assert!(
        SecureJobStore::with_test_key_and_profile(
            directory.path().join("jobs.enc"),
            CURRENT_KEY,
            PROFILE.as_bytes(),
        )
        .load()
        .is_err()
    );
}

#[test]
fn ordinary_transactions_cannot_clear_migrated_legacy_history() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path(), "rtrct02");
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let original = store.snapshot().unwrap();
    let original_bytes = fs::read(directory.path().join("jobs.enc")).unwrap();

    assert!(
        store
            .transaction(|state| {
                state.legacy_history.clear();
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original_bytes
    );
}

#[test]
fn ordinary_transactions_cannot_rewrite_migration_provenance() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path(), "rtrct02");
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let original = store.snapshot().unwrap();
    let original_bytes = fs::read(directory.path().join("jobs.enc")).unwrap();

    assert!(
        store
            .transaction(|state| {
                state.migration.as_mut().unwrap().source_sha256 =
                    format!("sha256:{}", "0".repeat(64));
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original_bytes
    );
}

#[test]
fn every_legacy_nonterminal_status_stops_without_an_inferred_account() {
    for (legacy_status, deleted, expected) in [
        (LegacyJobStatus::Queued, 0, LegacyTerminalStatus::Failed),
        (LegacyJobStatus::Running, 0, LegacyTerminalStatus::Failed),
        (LegacyJobStatus::Queued, 1, LegacyTerminalStatus::Partial),
        (LegacyJobStatus::Running, 1, LegacyTerminalStatus::Partial),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, fixture("rtrct02")).unwrap();
        let legacy_store =
            SecureJobStore::with_test_key_and_profile(path, CURRENT_KEY, PROFILE.as_bytes());
        let mut legacy = legacy_store.load().unwrap();
        legacy.jobs.truncate(1);
        legacy.jobs[0].status = legacy_status;
        legacy.jobs[0].deleted = deleted;
        legacy.jobs[0].next_batch = usize::from(deleted > 0);
        legacy.jobs[0].retry_after_seconds = Some(60);
        legacy_store.save(&legacy).unwrap();

        let state = FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
        )
        .unwrap()
        .snapshot()
        .unwrap();
        assert!(state.identities.is_empty());
        assert!(state.sources.is_empty());
        assert!(state.jobs.is_empty());
        assert_eq!(state.legacy_history[0].record.status, expected);
        assert_eq!(state.legacy_history[0].record.deleted, deleted as u64);
        assert!(!state.legacy_history[0].executable);
        assert!(
            state.legacy_history[0]
                .record
                .diagnostics
                .iter()
                .any(|error| {
                    error.code == ErrorCode::MigrationRequiresNewReview && error.retry_at.is_none()
                })
        );
    }
}

#[test]
fn every_legacy_terminal_status_is_preserved_as_nonexecutable_history() {
    for (legacy_status, expected) in [
        (LegacyJobStatus::Completed, LegacyTerminalStatus::Completed),
        (LegacyJobStatus::Partial, LegacyTerminalStatus::Partial),
        (LegacyJobStatus::Failed, LegacyTerminalStatus::Failed),
        (LegacyJobStatus::Cancelled, LegacyTerminalStatus::Cancelled),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, fixture("rtrct02")).unwrap();
        let legacy_store =
            SecureJobStore::with_test_key_and_profile(path, CURRENT_KEY, PROFILE.as_bytes());
        let mut legacy = legacy_store.load().unwrap();
        legacy.jobs.truncate(1);
        legacy.jobs[0].status = legacy_status;
        legacy_store.save(&legacy).unwrap();

        let state = FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
        )
        .unwrap()
        .snapshot()
        .unwrap();
        assert_eq!(state.legacy_history[0].record.status, expected);
        assert!(!state.legacy_history[0].executable);
        assert!(
            state.legacy_history[0]
                .record
                .diagnostics
                .iter()
                .all(|error| error.code != ErrorCode::MigrationRequiresNewReview)
        );
    }
}

#[test]
fn wrong_legacy_profile_unknown_headers_and_corrupt_ciphertext_fail_closed() {
    let wrong_profile_dir = tempfile::tempdir().unwrap();
    let original = write_fixture(wrong_profile_dir.path(), "rtrct02");
    let wrong = StoreBinding {
        provider: provider("telegram"),
        profile: "wrong-profile".into(),
    };
    assert!(
        FoundationStore::open_with_test_key(
            wrong_profile_dir.path().to_path_buf(),
            wrong,
            CURRENT_KEY,
        )
        .is_err()
    );
    assert_eq!(
        fs::read(wrong_profile_dir.path().join("jobs.enc")).unwrap(),
        original
    );
    assert!(
        !wrong_profile_dir
            .path()
            .join("jobs.pre-provider.enc")
            .exists()
    );

    for bytes in [b"RTRCT99unknown".to_vec(), {
        let mut bytes = fixture("rtrct01");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        bytes
    }] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("jobs.enc"), &bytes).unwrap();
        assert!(
            FoundationStore::open_with_test_key(
                directory.path().to_path_buf(),
                binding(),
                LEGACY_KEY,
            )
            .is_err()
        );
        assert_eq!(fs::read(directory.path().join("jobs.enc")).unwrap(), bytes);
        assert!(!directory.path().join("jobs.pre-provider.enc").exists());
    }
}

#[test]
fn missing_active_file_with_backup_or_candidate_artifact_requires_recovery() {
    for artifact in ["jobs.pre-provider.enc", "jobs.enc.tmp"] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(artifact), b"migration evidence").unwrap();

        assert!(
            FoundationStore::open_with_test_key(
                directory.path().to_path_buf(),
                binding(),
                CURRENT_KEY,
            )
            .is_err()
        );
        assert!(!directory.path().join("jobs.enc").exists());
    }
}

#[test]
fn a_conflicting_backup_is_never_overwritten_or_used_as_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let original = write_fixture(directory.path(), "rtrct02");
    let conflicting = b"different historical ciphertext".to_vec();
    fs::write(directory.path().join("jobs.pre-provider.enc"), &conflicting).unwrap();

    assert!(FoundationStore::open_with_test_key(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
    )
    .is_err());
    assert_eq!(
        fs::read(directory.path().join("jobs.pre-provider.enc")).unwrap(),
        conflicting
    );
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original
    );
}

#[test]
fn fresh_v3_files_are_private_and_authenticated_to_provider_and_profile() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    assert_eq!(store.snapshot().unwrap(), FoundationState::empty(binding()));
    drop(store);

    let active = directory.path().join("jobs.enc");
    assert!(fs::read(&active).unwrap().starts_with(b"RTRCT03"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&active).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let wrong_provider = StoreBinding {
        provider: provider("synthetic"),
        profile: PROFILE.into(),
    };
    let wrong_provider_error = decrypt_authenticated(
        &fs::read(&active).unwrap(),
        b"RTRCT03",
        &CURRENT_KEY,
        &store_aad(&wrong_provider).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(
        wrong_provider_error,
        AppError::SecureStore(message) if message == "job store authentication failed"
    ));
    assert!(
        FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            wrong_provider,
            CURRENT_KEY,
        )
        .is_err()
    );
    let wrong_profile = StoreBinding {
        provider: provider("telegram"),
        profile: "telegram-other".into(),
    };
    let wrong_profile_error = decrypt_authenticated(
        &fs::read(&active).unwrap(),
        b"RTRCT03",
        &CURRENT_KEY,
        &store_aad(&wrong_profile).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(
        wrong_profile_error,
        AppError::SecureStore(message) if message == "job store authentication failed"
    ));
    assert!(
        FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            wrong_profile,
            CURRENT_KEY,
        )
        .is_err()
    );
    FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
        .unwrap();
}

#[test]
fn callers_share_one_arc_but_an_independent_writer_gets_profile_in_use() {
    let directory = tempfile::tempdir().unwrap();
    let first =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let shared =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), [0x99; 32])
            .unwrap();
    assert!(Arc::ptr_eq(&first, &shared));

    let conflicting_binding = StoreBinding {
        provider: provider("synthetic"),
        profile: PROFILE.into(),
    };
    let error = FoundationStore::open_with_test_key(
        directory.path().to_path_buf(),
        conflicting_binding,
        CURRENT_KEY,
    )
    .unwrap_err();
    assert!(matches!(error, AppError::ProfileInUse));

    let error = FoundationStore::open_independent_with_test_key(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
    )
    .unwrap_err();
    assert!(matches!(error, AppError::ProfileInUse));
}

#[test]
fn an_independent_open_checks_the_profile_lock_before_fetching_the_key() {
    let directory = tempfile::tempdir().unwrap();
    let first =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let observed = fetches.clone();

    let error = FoundationStore::open_independent_with_test_key_loader(
        directory.path().to_path_buf(),
        binding(),
        move |_| {
            observed.fetch_add(1, Ordering::AcqRel);
            Ok(CURRENT_KEY)
        },
    )
    .unwrap_err();

    assert!(matches!(error, AppError::ProfileInUse));
    assert_eq!(fetches.load(Ordering::Acquire), 0);
    drop(first);
}

#[test]
fn transactions_validate_identity_ownership_plan_fingerprints_and_cursor_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    store
        .transaction(|state| {
            install_valid_graph(state);
            Ok(())
        })
        .unwrap();
    assert_eq!(store.snapshot().unwrap().jobs.len(), 1);

    let original = store.snapshot().unwrap();
    assert!(
        store
            .transaction(|state| {
                state.identities.push(state.identities[0].clone());
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);

    assert!(
        store
            .transaction(|state| {
                state.plans[0].fingerprint.push('0');
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);

    assert!(
        store
            .transaction(|state| {
                state.jobs[0].next_batch = 3;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);

    assert!(
        store
            .transaction(|state| {
                state.sources[0].account_id =
                    serde_json::from_value(json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")).unwrap();
                Ok(())
            })
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), original);
}

#[test]
fn two_account_ids_cannot_alias_one_verified_native_identity() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    let first = account();
    let mut alias = first.clone();
    alias.id = retract_domain::AccountId::try_from(
        Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap(),
    )
    .unwrap();

    assert!(
        store
            .transaction(|state| {
                state.identities.extend([first, alias]);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().identities.is_empty());
}

#[test]
fn adapter_validates_all_provider_payloads_and_unknown_recipe_jobs_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    assert!(
        store
            .transaction(|state| {
                let mut unsafe_account = account();
                unsafe_account.native_identity.payload["privateValue"] =
                    json!("SYNTHETIC_PRIVATE_CONTENT");
                state.identities.push(unsafe_account);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().identities.is_empty());

    assert!(
        store
            .transaction(|state| {
                state.identities.push(account());
                let mut unsafe_source = source();
                unsafe_source.schema_profile.payload["opaque"] = json!("SYNTHETIC_PRIVATE_CONTENT");
                state.sources.push(unsafe_source);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().sources.is_empty());

    for corrupt in ["private_payload", "canonical_disagreement"] {
        assert!(
            store
                .transaction(|state| {
                    let mut plan = plan();
                    for target in plan.targets.iter_mut().chain(
                        plan.steps
                            .iter_mut()
                            .flat_map(|step| step.targets.iter_mut()),
                    ) {
                        match corrupt {
                            "private_payload" => {
                                target.resource.locator_payload["privateValue"] =
                                    json!("SYNTHETIC_PRIVATE_CONTENT");
                            }
                            "canonical_disagreement" => {
                                target.resource.locator_payload["chatId"] = json!("-2000");
                            }
                            _ => unreachable!(),
                        }
                    }
                    plan.seal().unwrap();
                    let job = job(&plan);
                    state.identities.push(account());
                    state.sources.push(source());
                    state.plans.push(plan);
                    state.jobs.push(job);
                    Ok(())
                })
                .is_err()
        );
        assert!(store.snapshot().unwrap().plans.is_empty());
    }

    assert!(
        store
            .transaction(|state| {
                state.identities.push(account());
                state.sources.push(source());
                let mut unsafe_plan = plan();
                unsafe_plan.recipe.payload["opaque"] = json!("SYNTHETIC_PRIVATE_CONTENT");
                unsafe_plan.seal().unwrap();
                state.plans.push(unsafe_plan);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().plans.is_empty());

    assert!(
        store
            .transaction(|state| {
                state.identities.push(account());
                state.sources.push(source());
                let mut unknown = plan();
                unknown.recipe.version = 999;
                unknown.seal().unwrap();
                let job = job(&unknown);
                state.plans.push(unknown);
                state.jobs.push(job);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().jobs.is_empty());

    assert!(
        store
            .transaction(|state| {
                state.identities.push(account());
                state.sources.push(source());
                let mut mismatched = plan();
                mismatched.recipe.payload["targetKeys"][0] = json!("[\"-9999\",\"1\"]");
                mismatched.seal().unwrap();
                let job = job(&mismatched);
                state.plans.push(mismatched);
                state.jobs.push(job);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().jobs.is_empty());
}

#[test]
fn default_store_entry_is_fail_closed_for_provider_backed_v3_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    store
        .transaction(|state| {
            install_valid_graph(state);
            Ok(())
        })
        .unwrap();
    drop(store);

    assert!(
        FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
        )
        .is_err()
    );
}

#[test]
fn failed_private_write_never_publishes_the_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    let original_bytes = fs::read(directory.path().join("jobs.enc")).unwrap();
    fs::create_dir(directory.path().join("jobs.enc.tmp")).unwrap();

    assert!(matches!(
        store.transaction(|state| {
            state.identities.push(account());
            Ok(())
        }),
        Err(AppError::StatePersistenceFailed)
    ));
    assert!(store.snapshot().unwrap().identities.is_empty());
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original_bytes
    );
}

struct CorruptVerificationIo {
    real: RealStoreIo,
    armed: AtomicBool,
}

impl StoreIo for CorruptVerificationIo {
    fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        let mut bytes = self.real.read_candidate(path)?;
        if self.armed.swap(false, Ordering::AcqRel) {
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
        }
        Ok(bytes)
    }

    fn replace(&self, from: &Path, to: &Path) -> Result<(), AppError> {
        self.real.replace(from, to)
    }

    fn sync_directory(&self, path: &Path) -> Result<(), AppError> {
        self.real.sync_directory(path)
    }
}

#[test]
fn failed_candidate_verification_preserves_the_committed_snapshot_and_file() {
    let directory = tempfile::tempdir().unwrap();
    let io = Arc::new(CorruptVerificationIo {
        real: RealStoreIo,
        armed: AtomicBool::new(false),
    });
    let store = FoundationStore::open_with_test_key_and_io_and_payload_validator(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
        io.clone(),
        strict_validator(),
    )
    .unwrap();
    let original = fs::read(directory.path().join("jobs.enc")).unwrap();
    io.armed.store(true, Ordering::Release);

    assert!(matches!(
        store.transaction(|state| {
            state.identities.push(account());
            Ok(())
        }),
        Err(AppError::StatePersistenceFailed)
    ));
    assert!(store.snapshot().unwrap().identities.is_empty());
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original
    );
}

struct FailAfterReplaceIo {
    real: RealStoreIo,
    armed: AtomicBool,
}

impl StoreIo for FailAfterReplaceIo {
    fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        self.real.read_candidate(path)
    }

    fn replace(&self, from: &Path, to: &Path) -> Result<(), AppError> {
        self.real.replace(from, to)
    }

    fn sync_directory(&self, path: &Path) -> Result<(), AppError> {
        if self.armed.swap(false, Ordering::AcqRel) {
            return Err(io::Error::other("synthetic directory sync failure").into());
        }
        self.real.sync_directory(path)
    }
}

#[test]
fn a_post_replace_sync_failure_reloads_authoritative_disk_before_later_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let io = Arc::new(FailAfterReplaceIo {
        real: RealStoreIo,
        armed: AtomicBool::new(false),
    });
    let store = FoundationStore::open_with_test_key_and_io_and_payload_validator(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
        io.clone(),
        strict_validator(),
    )
    .unwrap();
    io.armed.store(true, Ordering::Release);

    assert!(matches!(
        store.transaction(|state| {
            state.identities.push(account());
            Ok(())
        }),
        Err(AppError::StatePersistenceFailed)
    ));

    store
        .transaction(|state| {
            assert_eq!(
                state.identities.len(),
                1,
                "must reload the replaced v3 file"
            );
            state.sources.push(source());
            Ok(())
        })
        .unwrap();
    let state = store.snapshot().unwrap();
    assert_eq!(state.identities.len(), 1);
    assert_eq!(state.sources.len(), 1);
}

#[test]
fn concurrent_transactions_never_overwrite_a_newer_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_validated(directory.path());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for index in 0..2 {
        let store = store.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut account = account();
            account.id = retract_domain::AccountId::try_from(Uuid::from_u128(100 + index)).unwrap();
            account.native_identity.payload["userId"] = json!((42 + index).to_string());
            barrier.wait();
            store.transaction(|state| {
                state.identities.push(account);
                Ok(state.identities.len())
            })
        }));
    }
    barrier.wait();
    let mut observed = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect::<Vec<_>>();
    observed.sort_unstable();
    assert_eq!(observed, vec![1, 2]);
    assert_eq!(store.snapshot().unwrap().identities.len(), 2);
}

#[test]
fn migration_replace_failure_leaves_the_authenticated_legacy_file_authoritative() {
    struct FailReplaceIo(RealStoreIo);
    impl StoreIo for FailReplaceIo {
        fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError> {
            self.0.read_candidate(path)
        }
        fn replace(&self, _from: &Path, _to: &Path) -> Result<(), AppError> {
            Err(io::Error::other("synthetic replace failure").into())
        }
        fn sync_directory(&self, path: &Path) -> Result<(), AppError> {
            self.0.sync_directory(path)
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let original = write_fixture(directory.path(), "rtrct02");
    assert!(
        FoundationStore::open_with_test_key_and_io(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
            Arc::new(FailReplaceIo(RealStoreIo)),
        )
        .is_err()
    );
    assert_eq!(
        fs::read(directory.path().join("jobs.enc")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(directory.path().join("jobs.pre-provider.enc")).unwrap(),
        original
    );
}

#[test]
fn interrupted_migration_write_or_verification_keeps_legacy_active_and_retries_safely() {
    let write_directory = tempfile::tempdir().unwrap();
    let write_original = write_fixture(write_directory.path(), "rtrct02");
    fs::create_dir(write_directory.path().join("jobs.enc.tmp")).unwrap();
    assert!(
        FoundationStore::open_with_test_key(
            write_directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
        )
        .is_err()
    );
    assert_eq!(
        fs::read(write_directory.path().join("jobs.enc")).unwrap(),
        write_original
    );
    assert_eq!(
        fs::read(write_directory.path().join("jobs.pre-provider.enc")).unwrap(),
        write_original
    );
    fs::remove_dir(write_directory.path().join("jobs.enc.tmp")).unwrap();
    FoundationStore::open_with_test_key(
        write_directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
    )
    .unwrap();

    let verify_directory = tempfile::tempdir().unwrap();
    let verify_original = write_fixture(verify_directory.path(), "rtrct02");
    let corrupting = Arc::new(CorruptVerificationIo {
        real: RealStoreIo,
        armed: AtomicBool::new(true),
    });
    assert!(
        FoundationStore::open_with_test_key_and_io(
            verify_directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
            corrupting,
        )
        .is_err()
    );
    assert_eq!(
        fs::read(verify_directory.path().join("jobs.enc")).unwrap(),
        verify_original
    );
    assert_eq!(
        fs::read(verify_directory.path().join("jobs.pre-provider.enc")).unwrap(),
        verify_original
    );
    FoundationStore::open_with_test_key(
        verify_directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
    )
    .unwrap();
}

#[test]
fn a_migration_post_replace_sync_failure_treats_v3_disk_as_authoritative_on_retry() {
    struct FailSecondSyncIo {
        real: RealStoreIo,
        calls: AtomicUsize,
    }
    impl StoreIo for FailSecondSyncIo {
        fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError> {
            self.real.read_candidate(path)
        }
        fn replace(&self, from: &Path, to: &Path) -> Result<(), AppError> {
            self.real.replace(from, to)
        }
        fn sync_directory(&self, path: &Path) -> Result<(), AppError> {
            if self.calls.fetch_add(1, Ordering::AcqRel) == 1 {
                return Err(io::Error::other("synthetic post-replace sync failure").into());
            }
            self.real.sync_directory(path)
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let original = write_fixture(directory.path(), "rtrct02");
    assert!(
        FoundationStore::open_with_test_key_and_io(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
            Arc::new(FailSecondSyncIo {
                real: RealStoreIo,
                calls: AtomicUsize::new(0),
            }),
        )
        .is_err()
    );
    assert!(
        fs::read(directory.path().join("jobs.enc"))
            .unwrap()
            .starts_with(b"RTRCT03")
    );
    assert_eq!(
        fs::read(directory.path().join("jobs.pre-provider.enc")).unwrap(),
        original
    );
    let state =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap()
            .snapshot()
            .unwrap();
    assert_eq!(state.legacy_history.len(), 2);
}

#[test]
fn legacy_operations_map_without_retaining_provider_native_targets() {
    let expected = [
        (
            PlanOperation::SelectedMessages,
            LegacyOperation::SelectedMessages,
        ),
        (
            PlanOperation::DeleteMyMessages,
            LegacyOperation::DeleteMyMessages,
        ),
        (PlanOperation::ClearHistory, LegacyOperation::ClearHistory),
        (
            PlanOperation::ClearHistoryAndLeave,
            LegacyOperation::ClearHistoryAndLeave,
        ),
        (
            PlanOperation::DeleteAllMessagesAndLeave,
            LegacyOperation::DeleteAllMessagesAndLeave,
        ),
        (
            PlanOperation::RemoveChatForSelf,
            LegacyOperation::RemoveChatForSelf,
        ),
        (
            PlanOperation::DeleteBySender,
            LegacyOperation::DeleteBySender,
        ),
        (PlanOperation::DeleteGroup, LegacyOperation::DeleteGroup),
        (PlanOperation::LeaveChat, LegacyOperation::LeaveChat),
    ];
    for (operation, expected) in expected {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, fixture("rtrct02")).unwrap();
        let legacy_store =
            SecureJobStore::with_test_key_and_profile(path, CURRENT_KEY, PROFILE.as_bytes());
        let mut legacy: PersistedState = legacy_store.load().unwrap();
        legacy.plans.truncate(1);
        legacy.jobs.truncate(1);
        legacy.plans[0].operation = operation;
        legacy.jobs[0].operation = operation;
        legacy_store.save(&legacy).unwrap();

        let state = FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            binding(),
            CURRENT_KEY,
        )
        .unwrap()
        .snapshot()
        .unwrap();
        assert_eq!(state.legacy_history[0].record.operation, expected);
        let encoded = serde_json::to_string(&state.legacy_history[0]).unwrap();
        assert!(!encoded.contains("targetChatIds"));
        assert!(!encoded.contains("target_chat_ids"));
    }
}
