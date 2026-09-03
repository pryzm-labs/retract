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
    LegacyOperation, LegacyTerminalStatus, ProviderKey, RemediationPlan, RestartPolicy, Scope,
    ScopedJobRecord, ScopedResourceRef, SourceRecord, VersionedPayload,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FoundationState, FoundationStore, LegacyStoreFormat, RealStoreIo, StoreBinding, StoreIo,
};
use crate::{
    error::AppError,
    model::{JobStatus as LegacyJobStatus, PersistedState},
    secure_store::SecureJobStore,
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
            payload: json!({"operation": "selected", "batchSize": 1}),
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
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
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
fn provider_recipes_are_content_free_and_unknown_versions_are_inertly_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    assert!(
        store
            .transaction(|state| {
                state.identities.push(account());
                state.sources.push(source());
                let mut unsafe_plan = plan();
                unsafe_plan.recipe.payload = json!({"messageBody": "SYNTHETIC_PRIVATE_CONTENT"});
                unsafe_plan.seal().unwrap();
                state.plans.push(unsafe_plan);
                Ok(())
            })
            .is_err()
    );
    assert!(store.snapshot().unwrap().plans.is_empty());

    store
        .transaction(|state| {
            state.identities.push(account());
            state.sources.push(source());
            let mut unknown = plan();
            unknown.recipe.version = 999;
            unknown.seal().unwrap();
            state.plans.push(unknown);
            Ok(())
        })
        .unwrap();
    let state = store.snapshot().unwrap();
    assert_eq!(state.plans[0].recipe.version, 999);
    assert!(
        state.jobs.is_empty(),
        "the foundation must not infer executable work"
    );
}

#[test]
fn failed_private_write_never_publishes_the_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
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
    let store = FoundationStore::open_with_test_key_and_io(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
        io.clone(),
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
    let store = FoundationStore::open_with_test_key_and_io(
        directory.path().to_path_buf(),
        binding(),
        CURRENT_KEY,
        io.clone(),
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
    let store =
        FoundationStore::open_with_test_key(directory.path().to_path_buf(), binding(), CURRENT_KEY)
            .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for index in 0..2 {
        let store = store.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut account = account();
            account.id = retract_domain::AccountId::try_from(Uuid::from_u128(100 + index)).unwrap();
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
