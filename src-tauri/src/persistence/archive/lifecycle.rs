//! Application-lifetime registration of lazy archive work and terminal drain.
use super::{ArchiveError, ArchiveService, ArchiveStore};
use std::sync::Arc;
use tokio::sync::Mutex;

type Opener = dyn Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync;

fn application_validators() -> std::collections::BTreeMap<
    retract_domain::ProviderKey,
    Arc<dyn crate::persistence::ProviderPayloadValidator>,
> {
    use crate::providers::discord::{DiscordPayloadValidator, locators::discord_provider_key};
    std::collections::BTreeMap::from([(
        discord_provider_key(),
        Arc::new(DiscordPayloadValidator) as Arc<dyn crate::persistence::ProviderPayloadValidator>,
    )])
}
pub(crate) struct ArchiveOwner {
    opener: Arc<Opener>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    service: Option<Arc<ArchiveService>>,
}

impl ArchiveOwner {
    pub(crate) fn unavailable() -> Self {
        Self::with_opener(|| Err(ArchiveError::UnavailableKey))
    }
    pub(crate) fn with_opener(
        opener: impl Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            opener: Arc::new(opener),
            state: Mutex::new(State::default()),
        }
    }

    pub(crate) fn application(root: std::path::PathBuf) -> Self {
        let validators = application_validators();
        Self::with_opener(move || {
            #[cfg(target_os = "macos")]
            {
                let path = prepare_archive_directory(&root)?.join("content.db");
                ArchiveStore::open_with_key_loader(path, validators.clone(), || {
                    crate::secure_store::load_archive_index_key().map_err(|error| match error {
                        crate::error::AppError::ProfileInUse => ArchiveError::StoreInUse,
                        _ => ArchiveError::UnavailableKey,
                    })
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (&root, &validators);
                Err(ArchiveError::UnavailableKey)
            }
        })
    }

    /// First-open failures are sticky to avoid repeated credential prompts/I/O.
    /// Correct the condition and restart the application; import retry and Busy
    /// backoff do not reset this application-lifetime factory.
    pub(crate) async fn open(&self) -> Result<Arc<ArchiveService>, ArchiveError> {
        let mut lifecycle = self.state.lock().await;
        if lifecycle.closed {
            return Err(ArchiveError::Cancelled);
        }
        let service = lifecycle.service.get_or_insert_with(|| {
            let factory = self.opener.clone();
            ArchiveService::start(move || factory())
        });
        service.wait_ready().await?;
        Ok(Arc::clone(service))
    }

    pub(crate) async fn shutdown(&self) {
        let mut lifecycle = self.state.lock().await;
        lifecycle.closed = true;
        if let Some(service) = lifecycle.service.as_ref() {
            service.shutdown().await;
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn prepare_archive_directory(
    root: &std::path::Path,
) -> Result<std::path::PathBuf, ArchiveError> {
    use std::{fs, io::ErrorKind, os::unix::fs::DirBuilderExt};
    // Create only beneath an existing canonical, non-writable parent; all work
    // runs on the worker. Never relax the repository's existing parent checks.
    if let Err(error) = fs::symlink_metadata(root) {
        if error.kind() != ErrorKind::NotFound {
            return Err(ArchiveError::InvalidStore);
        }
        super::store::validate_parent(root)?;
        match fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ArchiveError::StorageFailure),
        }
    }
    super::store::validate_parent(&root.join("archives"))?;
    let directory = root.join("archives");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(ArchiveError::StorageFailure),
    }
    super::store::validate_parent(&directory.join("content.db"))?;
    Ok(directory)
}

#[cfg(test)]
mod discord_tests {
    use super::*;
    use crate::persistence::archive::{ArchiveKey, ArchiveQuerySource, ImportBatch};
    use crate::providers::discord::{
        DiscordNormalizer, locators::discord_provider_key, model::DiscordSourceProfile,
    };
    use crate::providers::ports::{ContentQuery, QuerySource};
    use discord_archive::{ChannelContext, DiscordId, ExportAccount, SentMessage};
    use retract_domain::*;

    use super::super::worker_tests::{Gate, Release, runtime};
    use crate::providers::discord::import::{DiscordImportError, DiscordImportOwner, TestPoint};
    use crate::providers::discord::progress::DiscordImportPhase;
    use std::{
        fs::{self, File},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    const ZIP: &[u8] = include_bytes!("../../../test-fixtures/discord-import/current-json.zip");

    #[test]
    fn discord_import_receipts_match_exact_v2_batches_and_warning_order() {
        use super::super::{ImportBatchV2, ImportWarningCode, ImportWarningDelta};
        use sha2::{Digest, Sha256};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content.db");
        let selected = directory.path().join("selected.zip");
        fs::write(
            &selected,
            crate::providers::discord::import_tests::package(700, "exact receipt", false),
        )
        .unwrap();
        let store_path = path.clone();
        let archives = Arc::new(ArchiveOwner::with_opener(move || {
            ArchiveStore::open(
                store_path.clone(),
                ArchiveKey::new([0x85; 32]),
                application_validators(),
            )
        }));
        runtime().block_on(async {
            let imports = DiscordImportOwner::new(archives.clone());
            let result = imports.start(File::open(&selected).unwrap()).await.unwrap().wait().await.unwrap();
            imports.shutdown().await;
            archives.shutdown().await;
            let scope = result.checkpoint.scope;
            let normalizer = DiscordNormalizer::new(scope, result.checkpoint.observed_at).unwrap();
            let account = ExportAccount { id: DiscordId::parse("9007199254741001").unwrap(), username: "invented_owner".into() };
            let channel = ChannelContext { id: DiscordId::parse("9007199254741101").unwrap(), source_type: "invented_kind".into(), name: None, recipients: None, guild: None };
            let mut first = ImportBatchV2 { records: ImportBatch { actors: vec![normalizer.actor(&account).unwrap()], conversations: vec![normalizer.conversation(&channel).unwrap()], contents: vec![] }, warnings: vec![ImportWarningDelta { code: ImportWarningCode::UnknownConversationKind, count: 1 }] };
            let mut second = ImportBatchV2::default();
            for n in 0..700 {
                let content = normalizer.content(&account, &channel, &SentMessage { id: DiscordId::parse(&(1985931830091579392u64 + n).to_string()).unwrap(), account_id: account.id.clone(), channel_id: channel.id.clone(), timestamp_millis: 1_893_553_445_123, contents: "exact receipt".into(), attachments: String::new() }).unwrap();
                if n < 498 { first.records.contents.push(content); } else { second.records.contents.push(content); }
            }
            let store = ArchiveStore::open(path.clone(), ArchiveKey::new([0x85;32]), application_validators()).unwrap();
            let db = store.connection.lock().unwrap();
            let mut bytes = 0;
            for (sequence, expected) in [first, second].iter().enumerate() {
                let encoded = serde_json::to_vec(&("retract.archive.batch", 2_u8, &expected.records, &expected.warnings)).unwrap();
                bytes += encoded.len() as i64;
                let digest: String = Sha256::digest(&encoded).iter().map(|b| format!("{b:02x}")).collect();
                let stored = db.query_row("SELECT digest, digest_version, committed_records, committed_bytes, next_batch FROM import_batch_receipts WHERE sequence=?", [sequence as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))).unwrap();
                assert_eq!(stored, (digest, 2, if sequence == 0 {498} else {700}, bytes, sequence as i64 + 1));
            }
        });
    }

    #[test]
    fn discord_import_cancel_during_commit_rolls_back_and_full_queue_cancels_without_late_writes() {
        for full_queue in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("content.db");
            let selected = directory.path().join("selected.zip");
            fs::write(&selected, ZIP).unwrap();
            let gate = Arc::new(Gate::default());
            let _release = Release(gate.clone());
            let commit_gate = gate.clone();
            let archives = Arc::new(ArchiveOwner::with_opener(move || {
                let mut store = ArchiveStore::open(
                    path.clone(),
                    ArchiveKey::new([0x85; 32]),
                    application_validators(),
                )?;
                if !full_queue {
                    let gate = commit_gate.clone();
                    store.before_commit = Some(Box::new(move || gate.wait()));
                }
                Ok(store)
            }));
            let reached_queue = Arc::new(AtomicBool::new(false));
            let reached = reached_queue.clone();
            let registered = gate.clone();
            let imports = DiscordImportOwner::with_hook(archives.clone(), move |point| {
                if point == TestPoint::Registered {
                    registered.armed.store(true, Ordering::Release);
                    if full_queue {
                        registered.wait();
                    }
                }
                if point == TestPoint::QueueBusy {
                    reached.store(true, Ordering::Release);
                }
            });
            runtime().block_on(async {
                let handle = imports.start(File::open(&selected).unwrap()).await.unwrap();
                gate.entered().await;
                if full_queue {
                    let service = archives.open().await.unwrap();
                    let first = super::super::worker::reserve_command_slot(&service);
                    let second = super::super::worker::reserve_command_slot(&service);
                    gate.release();
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while !reached_queue.load(Ordering::Acquire) {
                            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        }
                    })
                    .await
                    .unwrap();
                    handle.cancel();
                    drop(first);
                    drop(second);
                } else {
                    handle.cancel();
                    gate.release();
                }
                assert_eq!(
                    handle.wait().await.err(),
                    Some(DiscordImportError::Cancelled)
                );
                assert_eq!(handle.checkpoint().unwrap().progress.committed_items, 0);
                imports.shutdown().await;
                archives.shutdown().await;
            });
        }
    }

    #[test]
    fn discord_import_disk_full_and_finalization_failure_keep_partial_source_hidden() {
        for finalization in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("content.db");
            let selected = directory.path().join("selected.zip");
            fs::write(
                &selected,
                if finalization {
                    ZIP.to_vec()
                } else {
                    crate::providers::discord::import_tests::package(
                        4,
                        &"x".repeat(64 * 1024),
                        false,
                    )
                },
            )
            .unwrap();
            let archives = Arc::new(ArchiveOwner::with_opener(move || {
                let store = ArchiveStore::open(
                    path.clone(),
                    ArchiveKey::new([0x85; 32]),
                    application_validators(),
                )?;
                {
                    let db = store.connection.lock().unwrap();
                    if finalization {
                        db.execute_batch("CREATE TEMP TRIGGER synthetic_full BEFORE UPDATE OF state ON import_runs WHEN NEW.state='ready' BEGIN SELECT RAISE(ABORT, 'synthetic finalization failure'); END;").unwrap();
                    } else {
                        let pages: i64 =
                            db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
                        db.pragma_update(None, "max_page_count", pages + 10)
                            .unwrap();
                    }
                }
                Ok(store)
            }));
            runtime().block_on(async {
                let imports = DiscordImportOwner::new(archives.clone());
                let handle = imports.start(File::open(&selected).unwrap()).await.unwrap();
                assert_eq!(
                    handle.wait().await.err(),
                    Some(DiscordImportError::StorageFailure)
                );
                let checkpoint = handle.checkpoint().unwrap();
                assert_eq!(checkpoint.progress.phase, super::super::ImportPhase::Failed);
                assert_eq!(
                    checkpoint.progress.committed_items,
                    if finalization { 4 } else { 0 }
                );
                assert_eq!(
                    checkpoint.failure_code,
                    Some(super::super::ImportFailureCode::StorageFailure)
                );
                let service = archives.open().await.unwrap();
                assert_ne!(
                    service.source(&checkpoint.scope).await.unwrap().state,
                    SourceState::Ready
                );
                imports.shutdown().await;
                archives.shutdown().await;
            });
        }
    }

    #[test]
    fn discord_import_runtime_shutdown_drains_parser_then_archive_then_secrets_in_every_phase() {
        for point in [
            TestPoint::BeforeLaunch,
            TestPoint::Poll(DiscordImportPhase::Inspecting),
            TestPoint::HashChunk,
            TestPoint::Registered,
            TestPoint::Poll(DiscordImportPhase::Importing),
            TestPoint::VerifyHashChunk,
            TestPoint::BeforeFinish,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("content.db");
            let selected = directory.path().join("selected.zip");
            fs::write(&selected, ZIP).unwrap();
            let gate = Arc::new(Gate::default());
            gate.armed.store(true, Ordering::Release);
            let _release = Release(gate.clone());
            let hold = gate.clone();
            let calls = Arc::new(AtomicUsize::new(0));
            let attempts = calls.clone();
            let archives = Arc::new(ArchiveOwner::with_opener(move || {
                attempts.fetch_add(1, Ordering::AcqRel);
                ArchiveStore::open(
                    path.clone(),
                    ArchiveKey::new([0x85; 32]),
                    application_validators(),
                )
            }));
            let imports = DiscordImportOwner::with_hook(archives.clone(), move |seen| {
                if seen == point {
                    hold.wait();
                }
            });
            runtime().block_on(async {
                let application = crate::RuntimeState {
                    service: tokio::sync::RwLock::new(
                        crate::provider_service::ProviderService::setup(),
                    ),
                    archives: archives.clone(),
                    discord_imports: imports,
                };
                let handle = application
                    .discord_imports
                    .start(File::open(&selected).unwrap())
                    .await
                    .unwrap();
                gate.entered().await;
                let cleared = AtomicBool::new(false);
                let clear = || {
                    assert!(application.discord_imports.is_drained());
                    assert!(archives.state.try_lock().unwrap().closed);
                    assert!(handle.latest_progress().phase != DiscordImportPhase::Ready);
                    cleared.store(true, Ordering::Release);
                };
                let mut shutdown = Box::pin(application.shutdown(clear));
                assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
                assert!(
                    !archives.state.try_lock().unwrap().closed,
                    "archive closed before parser drained: {point:?}"
                );
                assert!(!cleared.load(Ordering::Acquire));
                assert_eq!(
                    application
                        .discord_imports
                        .start(File::open(&selected).unwrap())
                        .await
                        .err(),
                    Some(DiscordImportError::Closed)
                );
                drop(shutdown);
                gate.release();
                application.shutdown(clear).await;
                assert!(cleared.load(Ordering::Acquire));
                assert_eq!(
                    handle.wait().await.err(),
                    Some(DiscordImportError::Cancelled)
                );
                if point == TestPoint::BeforeLaunch {
                    assert_eq!(calls.load(Ordering::Acquire), 0);
                }
            });
        }
    }

    #[test]
    fn discord_import_shutdown_during_key_open_retains_worker_and_clears_only_after_lock_release() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content.db");
        let selected = directory.path().join("selected.zip");
        fs::write(&selected, ZIP).unwrap();
        let gate = Arc::new(Gate::default());
        gate.armed.store(true, Ordering::Release);
        let _release = Release(gate.clone());
        let hold = gate.clone();
        let worker_path = path.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let attempts = calls.clone();
        let archives = ArchiveOwner::with_opener(move || {
            attempts.fetch_add(1, Ordering::AcqRel);
            hold.wait();
            ArchiveStore::open(
                worker_path.clone(),
                ArchiveKey::new([0x85; 32]),
                application_validators(),
            )
        });
        runtime().block_on(async {
            let application = crate::RuntimeState::with_archives(
                crate::provider_service::ProviderService::setup(),
                archives,
            );
            let handle = application
                .discord_imports
                .start(File::open(&selected).unwrap())
                .await
                .unwrap();
            gate.entered().await;
            assert_eq!(
                handle.latest_progress().phase,
                DiscordImportPhase::Registering
            );
            let cleared = AtomicBool::new(false);
            let clear = || {
                assert!(application.discord_imports.is_drained());
                // The keyed repository lock must already be released before clearing.
                drop(
                    ArchiveStore::open(
                        path.clone(),
                        ArchiveKey::new([0x85; 32]),
                        application_validators(),
                    )
                    .unwrap(),
                );
                cleared.store(true, Ordering::Release);
            };
            let mut shutdown = Box::pin(application.shutdown(clear));
            assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
            drop(shutdown);
            assert!(!cleared.load(Ordering::Acquire));
            gate.release();
            application.shutdown(clear).await;
            assert_eq!(
                handle.wait().await.err(),
                Some(DiscordImportError::Cancelled)
            );
            assert!(cleared.load(Ordering::Acquire));
            assert_eq!(calls.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn discord_application_validator_registration_is_lazy_and_does_not_offer_a_provider() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("absent");
        let owner = ArchiveOwner::application(root.clone());
        let state = owner.state.try_lock().unwrap();
        assert!(state.service.is_none()); // No worker/opener can access the DB or key loader.
        assert!(!root.exists());
        let validators = application_validators();
        assert_eq!(validators.len(), 1);
        assert_eq!(
            validators[&discord_provider_key()]
                .validation_policy_key()
                .as_str(),
            "discord.archive_payload.v1"
        );
        assert!(
            crate::providers::ProviderRegistry::default()
                .get(&discord_provider_key())
                .is_err()
        );
    }

    #[test]
    fn discord_archive_reopens_source_observations_and_derives_shared_privacy_findings() {
        let directory = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(directory.path())
            .unwrap()
            .join("content.db");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let observed = "2031-01-02T03:04:05.123Z".parse().unwrap();
            let account_input = ExportAccount {
                id: DiscordId::parse("9007199254741001").unwrap(),
                username: "invented_owner".into(),
            };
            let channel = ChannelContext {
                id: DiscordId::parse("9007199254741101").unwrap(),
                source_type: "opaque".into(),
                name: Some("Invented room".into()),
                recipients: None,
                guild: None,
            };
            let mut message = SentMessage {
                id: DiscordId::parse("1985931830091579393").unwrap(),
                account_id: account_input.id.clone(),
                channel_id: channel.id.clone(),
                timestamp_millis: 1_893_553_445_123,
                contents: "owner@example.test".into(),
                attachments: "https://example.invalid/passport.png".into(),
            };
            let make_owner = || {
                let path = path.clone();
                ArchiveOwner::with_opener(move || {
                    ArchiveStore::open(
                        path.clone(),
                        ArchiveKey::new([0x85; 32]),
                        application_validators(),
                    )
                })
            };
            let owner = make_owner();
            let service = owner.open().await.unwrap();
            let mut expected = vec![];
            for source_number in [2, 3] {
                let scope = Scope {
                    provider: discord_provider_key(),
                    account_id: uuid::Uuid::from_u128(1).try_into().unwrap(),
                    source_id: uuid::Uuid::from_u128(source_number).try_into().unwrap(),
                };
                let normalizer = DiscordNormalizer::new(scope.clone(), observed).unwrap();
                let account = normalizer.account(&account_input).unwrap();
                let source = SourceRecord {
                    id: scope.source_id,
                    account_id: scope.account_id,
                    provider: discord_provider_key(),
                    kind: SourceKind::ArchiveImport,
                    state: SourceState::Preparing,
                    archive_fingerprint: Some(format!("synthetic-{source_number}")),
                    schema_profile: DiscordSourceProfile::payload(),
                    imported_at: None,
                    updated_at: observed,
                    warnings: vec![],
                };
                let record = normalizer
                    .content(&account_input, &channel, &message)
                    .unwrap();
                service.register_source(&account, &source).await.unwrap();
                let session = service.begin_import(&scope).await.unwrap();
                service
                    .append_batch(
                        &session,
                        0,
                        &ImportBatch {
                            conversations: vec![normalizer.conversation(&channel).unwrap()],
                            actors: vec![normalizer.actor(&account_input).unwrap()],
                            contents: vec![record.clone()],
                        },
                    )
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                service.finish_import(&session).await.unwrap();
                expected.push(record);
                message.contents = "new observation".into();
                message.attachments = "https://example.invalid/second.png".into();
            }
            owner.shutdown().await;
            let owner = make_owner();
            let service = owner.open().await.unwrap();
            let query = ArchiveQuerySource(service.clone());
            for (index, expected) in expected.iter().enumerate() {
                let found = query
                    .search(ContentQuery {
                        scope: expected.scope.clone(),
                        query: String::new(),
                        cursor: None,
                        limit: 200,
                    })
                    .await
                    .unwrap()
                    .items;
                assert_eq!(found.len(), 1);
                let mut record = found[0].clone();
                if index == 0 {
                    assert!(record.privacy_findings.contains(&PrivacyKind::EmailAddress));
                    assert!(
                        record
                            .privacy_findings
                            .contains(&PrivacyKind::IdentityDocument)
                    );
                } else {
                    assert!(record.privacy_findings.is_empty());
                }
                assert!(
                    record
                        .detector_version
                        .as_deref()
                        .unwrap()
                        .starts_with("cleaner-sha256:")
                );
                record.privacy_findings.clear();
                record.detector_version = None;
                assert_eq!(&record, expected);
            }
            assert_eq!(expected[0].id, expected[1].id);
            assert_eq!(expected[0].resource, expected[1].resource);
            assert_eq!(
                service
                    .remove_source(&expected[0].scope)
                    .await
                    .unwrap()
                    .removed_items,
                1
            );
            assert_eq!(
                query
                    .search(ContentQuery {
                        scope: expected[1].scope.clone(),
                        query: "new observation".into(),
                        cursor: None,
                        limit: 200
                    })
                    .await
                    .unwrap()
                    .items
                    .len(),
                1
            );
            owner.shutdown().await;
        });
    }
}
