//! Synthetic end-to-end coordinator contracts; no external archive or key.
use super::import::{DiscordImportHandle, DiscordImportOutcome, DiscordImportOwner};
use super::progress::DiscordImportPhase;
use crate::persistence::archive::{
    ArchiveKey, ArchiveOwner, ArchiveSearch, ArchiveService, ArchiveStore, ImportDisposition,
};
use retract_domain::{ContentRecord, PrivacyKind, SourceState};
use std::{
    fs::{self, File},
    path::Path,
    sync::Arc,
};

// Test-only launch adapter: storage-internal lifecycle assertions stay beside
// the storage implementation without exposing production import capabilities.
pub(crate) trait ImportTestLaunch {
    async fn start_for_test(&self, file: File) -> Result<DiscordImportHandle, DiscordImportError>;
    async fn retry_for_test(
        &self,
        file: File,
        expected: &DiscordImportOutcome,
    ) -> Result<DiscordImportHandle, DiscordImportError>;
}

impl ImportTestLaunch for DiscordImportOwner {
    async fn start_for_test(&self, file: File) -> Result<DiscordImportHandle, DiscordImportError> {
        self.start(file).await
    }

    async fn retry_for_test(
        &self,
        file: File,
        expected: &DiscordImportOutcome,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        self.retry(file, expected).await
    }
}

#[path = "../../../../crates/discord-archive/tests/common/mod.rs"]
mod zip_fixture;

const FROZEN: &[u8] = include_bytes!("../../../test-fixtures/discord-import/current-json.zip");
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}
fn archive(path: &Path) -> Arc<ArchiveOwner> {
    let path = path.to_owned();
    Arc::new(ArchiveOwner::with_opener(move || {
        ArchiveStore::open(
            path.clone(),
            ArchiveKey::new([0x85; 32]),
            std::collections::BTreeMap::from([(
                super::locators::discord_provider_key(),
                Arc::new(super::DiscordPayloadValidator)
                    as Arc<dyn crate::persistence::ProviderPayloadValidator>,
            )]),
        )
    }))
}
fn selected(path: &Path, bytes: &[u8]) -> File {
    fs::write(path, bytes).unwrap();
    File::open(path).unwrap()
}
pub(crate) fn package(rows: usize, text: &str, invalid_tail: bool) -> Vec<u8> {
    let messages = (0..rows)
        .map(|n| {
            format!(
                r#"{{"ID":{},"Timestamp":"2030-01-02 03:04:05","Contents":{},"Attachments":""}}"#,
                1985931830091579392u64 + n as u64,
                serde_json::to_string(text).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let transcript = format!(
        "[{messages}{}]",
        if invalid_tail {
            r#",{"ID":1985931830091580392,"Timestamp":"2030-01-02 03:04:06","Contents":"invalid timestamp relation","Attachments":""}"#
        } else {
            ""
        }
    );
    zip_fixture::zip(&[
        (
            "Account/user.json",
            br#"{"id":"9007199254741001","username":"invented_owner"}"#,
        ),
        ("Messages/index.json", b"{}"),
        (
            "Messages/c9007199254741101/channel.json",
            br#"{"id":"9007199254741101","type":"invented_kind"}"#,
        ),
        (
            "Messages/c9007199254741101/messages.json",
            transcript.as_bytes(),
        ),
    ])
}
async fn query(service: &ArchiveService, outcome: &DiscordImportOutcome) -> Vec<ContentRecord> {
    let mut request = ArchiveSearch {
        scope: outcome.checkpoint.scope.clone(),
        text: String::new(),
        kinds: vec![],
        author: None,
        before: None,
        after: None,
        cursor: None,
        limit: 1,
    };
    let mut rows = vec![];
    loop {
        let page = service.search(&request).await.unwrap();
        rows.extend(page.items);
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            return rows;
        }
    }
}

// Catches omitted messages, readiness before integrity, unstable identities,
// shared observations, non-idempotent resolution, and writes to Telegram state.
#[test]
fn discord_import_frozen_snapshot_restart_identity_privacy_and_removal() {
    runtime().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let telegram = root.join("telegram");
        fs::create_dir(&telegram).unwrap();
        let context = crate::compatibility::fixtures::context("context");
        let (_harness, _) =
            crate::compatibility::fixtures::telegram(&telegram, context.clone()).await;
        let telegram_before = crate::compatibility::fixtures::encrypted_state(
            &telegram,
            &context,
            crate::compatibility::fixtures::KEY,
        )
        .unwrap();
        let path = root.join("content.db");
        let archives = archive(&path);
        let imports = DiscordImportOwner::new(archives.clone());
        assert!(!path.exists());
        let handle = imports
            .start(selected(&root.join("selected.zip"), FROZEN))
            .await
            .unwrap();
        let first = handle.wait().await.unwrap();
        assert_eq!(handle.latest_progress().phase, DiscordImportPhase::Ready);
        assert_eq!(handle.latest_progress().parsed_records, 4);
        assert_eq!(handle.latest_progress().processed_entries, 12);
        assert_eq!(
            handle.latest_progress().hashed_bytes,
            2 * FROZEN.len() as u64
        );
        assert_eq!(handle.latest_progress().total_records, Some(4));
        assert_eq!(first.checkpoint.progress.committed_items, 4);
        assert_eq!(first.checkpoint.progress.next_batch, 1);
        assert_eq!(
            first
                .checkpoint
                .warnings
                .iter()
                .map(|w| (w.code.as_str(), w.count))
                .collect::<Vec<_>>(),
            [("unknown_conversation_kind", 5)]
        );
        let service = archives.open().await.unwrap();
        assert_eq!(
            service.source(&first.checkpoint.scope).await.unwrap().state,
            SourceState::Ready
        );
        let rows = query(&service, &first).await;
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.privacy_findings.is_empty()));
        assert!(
            rows.iter()
                .any(|r| r.searchable_text == "Invented hello 🌱\nSecond invented line.")
        );
        let repeat = imports
            .start(File::open(root.join("selected.zip")).unwrap())
            .await
            .unwrap();
        assert_eq!(repeat.wait().await.unwrap().checkpoint, first.checkpoint);
        assert_eq!(repeat.latest_progress().parsed_records, 0);
        imports.shutdown().await;
        archives.shutdown().await;
        let archives = archive(&path);
        let imports = DiscordImportOwner::new(archives.clone());
        let again = imports
            .start(File::open(root.join("selected.zip")).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(again.disposition, ImportDisposition::Ready);
        assert_eq!(again.checkpoint, first.checkpoint);
        let service = archives.open().await.unwrap();
        assert_eq!(query(&service, &again).await, rows);
        let snapshot = imports
            .start(selected(
                &root.join("new.zip"),
                &package(2, "owner@example.test", false),
            ))
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_ne!(
            snapshot.checkpoint.scope.source_id,
            first.checkpoint.scope.source_id
        );
        assert_eq!(
            snapshot.checkpoint.scope.account_id,
            first.checkpoint.scope.account_id
        );
        let newer = query(&service, &snapshot).await;
        assert_eq!(newer.len(), 2);
        assert!(
            newer
                .iter()
                .all(|r| r.privacy_findings.contains(&PrivacyKind::EmailAddress))
        );
        let same_id = newer
            .iter()
            .find(|r| rows.iter().any(|old| old.id == r.id))
            .unwrap();
        assert_ne!(
            same_id.searchable_text,
            rows.iter()
                .find(|r| r.id == same_id.id)
                .unwrap()
                .searchable_text
        );
        assert_eq!(
            service
                .remove_source(&first.checkpoint.scope)
                .await
                .unwrap()
                .removed_items,
            4
        );
        assert_eq!(query(&service, &snapshot).await, newer);
        assert_eq!(
            serde_json::to_value(
                crate::compatibility::fixtures::encrypted_state(
                    &telegram,
                    &context,
                    crate::compatibility::fixtures::KEY
                )
                .unwrap()
            )
            .unwrap(),
            serde_json::to_value(telegram_before).unwrap()
        );
        imports.shutdown().await;
        archives.shutdown().await;
        let ciphertext = fs::read(&path).unwrap();
        assert!(!ciphertext.starts_with(b"SQLite format 3"));
        assert!(
            !ciphertext
                .windows(b"owner@example.test".len())
                .any(|part| part == b"owner@example.test")
        );
    });
}

#[test]
fn discord_import_enforces_combined_inspection_and_typed_read_budget() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let bytes = package(700, "synthetic text", false);
        let limits = discord_archive::ArchiveLimits {
            max_observed_bytes: bytes.len() as u64 * 3 / 2,
            ..Default::default()
        };
        let archives = archive(&dir.path().join("content.db"));
        let imports = DiscordImportOwner::with_limits(archives.clone(), limits);
        let handle = imports
            .start(selected(&dir.path().join("selected.zip"), &bytes))
            .await
            .unwrap();
        assert_eq!(
            handle.wait().await.err(),
            Some(DiscordImportError::LimitExceeded)
        );
        assert_eq!(
            handle.checkpoint().unwrap().progress.phase,
            ImportPhase::Failed
        );
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

#[test]
fn discord_import_installation_precedes_parser_and_key_work_and_stable_hardlinks_are_accepted() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let installed = Arc::new(AtomicBool::new(false));
        let parsing = installed.clone();
        let before_launch = Arc::new(Gate::default());
        let waiting = before_launch.clone();
        let path = dir.path().join("selected.zip");
        let file = selected(&path, FROZEN);
        fs::hard_link(&path, dir.path().join("stable-link.zip")).unwrap();
        let key_installed = installed.clone();
        let archive_path = dir.path().join("content.db");
        let archives = Arc::new(ArchiveOwner::with_opener(move || {
            assert!(key_installed.load(Ordering::Acquire));
            ArchiveStore::open(
                archive_path.clone(),
                ArchiveKey::new([0x85; 32]),
                std::collections::BTreeMap::from([(
                    super::locators::discord_provider_key(),
                    Arc::new(super::DiscordPayloadValidator)
                        as Arc<dyn crate::persistence::ProviderPayloadValidator>,
                )]),
            )
        }));
        let imports = DiscordImportOwner::with_hook(archives.clone(), move |point| {
            if point == TestPoint::BeforeLaunch {
                waiting.release();
            } else if point == TestPoint::Installed {
                before_launch.stop();
                parsing.store(true, Ordering::Release);
            } else {
                assert!(
                    parsing.load(Ordering::Acquire),
                    "work preceded owner installation"
                );
            }
        });
        let handle = imports.start(file).await.unwrap();
        let result = handle.wait().await.unwrap();
        assert_eq!(result.checkpoint.progress.committed_items, 4);
        let diagnostic = format!("{:?}", handle.latest_progress());
        for private in [
            "invented_owner",
            "900719925474",
            "Invented",
            "https:",
            "selected.zip",
            "discord.coordinator",
        ] {
            assert!(!diagnostic.contains(private));
        }
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

use super::import::{DiscordImportError, TestPoint};

#[test]
fn discord_import_failure_diagnostics_exclude_synthetic_content_and_selected_path() {
    runtime().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let archives = archive(&root.join("content.db"));
        let imports = DiscordImportOwner::new(archives.clone());
        let sentinel = "SYNTHETIC_DIAGNOSTIC_SENTINEL@example.invalid";
        let path = root.join("synthetic-diagnostic-sentinel.zip");
        let handle = imports
            .start(selected(&path, &package(3, sentinel, true)))
            .await
            .unwrap();
        let error = handle.wait().await.unwrap_err();
        assert_eq!(error, DiscordImportError::InvalidArchive);
        let diagnostic = format!("{error:?} {error} {:?}", handle.latest_progress());
        for excluded in [
            sentinel,
            "synthetic-diagnostic-sentinel",
            root.to_str().unwrap(),
            "198593183009",
            "invented_owner",
        ] {
            assert!(!diagnostic.contains(excluded));
        }
        assert_eq!(handle.latest_progress().phase, DiscordImportPhase::Failed);
        imports.shutdown().await;
        archives.shutdown().await;
    });
}
use crate::persistence::archive::{ArchiveError, ImportPhase};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn stop(&self) {
        self.entered.store(true, Ordering::Release);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
    async fn entered(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !self.entered.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn paused(
    archives: Arc<ArchiveOwner>,
    point: TestPoint,
) -> (DiscordImportOwner, Arc<Gate>, Release) {
    paused_after(archives, point, 1)
}
fn paused_after(
    archives: Arc<ArchiveOwner>,
    point: TestPoint,
    nth: usize,
) -> (DiscordImportOwner, Arc<Gate>, Release) {
    let gate = Arc::new(Gate::default());
    let hold = gate.clone();
    let seen_count = AtomicUsize::new(0);
    let owner = DiscordImportOwner::with_hook(archives, move |seen| {
        if seen == point && seen_count.fetch_add(1, Ordering::AcqRel) + 1 == nth {
            hold.stop();
        }
    });
    (owner, gate.clone(), Release(gate))
}

// Removing cancellation polling from any phase must fail this deterministic test.
#[test]
fn discord_import_cancellation_inventory_both_hashes_json_and_finalization() {
    for point in [
        TestPoint::BeforeRead,
        TestPoint::Poll(DiscordImportPhase::Inspecting),
        TestPoint::HashChunk,
        TestPoint::Poll(DiscordImportPhase::Importing),
        TestPoint::VerifyHashChunk,
        TestPoint::BeforeFinish,
    ] {
        runtime().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let archives = archive(&dir.path().join("content.db"));
            let (imports, gate, _release) = paused(archives.clone(), point);
            let handle = imports
                .start(selected(&dir.path().join("selected.zip"), FROZEN))
                .await
                .unwrap();
            gate.entered().await;
            handle.cancel();
            gate.release();
            assert_eq!(
                handle.wait().await.err(),
                Some(DiscordImportError::Cancelled),
                "{point:?}"
            );
            assert_eq!(
                handle.latest_progress().phase,
                DiscordImportPhase::Cancelled
            );
            if let Some(checkpoint) = handle.checkpoint() {
                assert_eq!(checkpoint.progress.phase, ImportPhase::Cancelled);
                let service = archives.open().await.unwrap();
                assert_eq!(
                    service
                        .search(&ArchiveSearch {
                            scope: checkpoint.scope,
                            text: String::new(),
                            kinds: vec![],
                            author: None,
                            before: None,
                            after: None,
                            cursor: None,
                            limit: 1
                        })
                        .await
                        .err(),
                    Some(ArchiveError::IncompleteSource)
                );
            }
            imports.shutdown().await;
            archives.shutdown().await;
        });
    }
}

#[test]
fn discord_import_pre_registration_storage_loss_is_not_user_cancellation() {
    for point in [
        TestPoint::Poll(DiscordImportPhase::Inspecting),
        TestPoint::HashChunk,
    ] {
        for stop_owner in [false, true] {
            runtime().block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("content.db");
                let calls = Arc::new(AtomicUsize::new(0));
                let opened = calls.clone();
                let archives = Arc::new(ArchiveOwner::with_opener(move || {
                    opened.fetch_add(1, Ordering::AcqRel);
                    ArchiveStore::open(
                        path.clone(),
                        ArchiveKey::new([0x85; 32]),
                        std::collections::BTreeMap::from([(
                            super::locators::discord_provider_key(),
                            Arc::new(super::DiscordPayloadValidator)
                                as Arc<dyn crate::persistence::ProviderPayloadValidator>,
                        )]),
                    )
                }));
                let service = archives.open().await.unwrap();
                let (imports, gate, _release) = paused(archives.clone(), point);
                let handle = imports
                    .start(selected(&dir.path().join("selected.zip"), FROZEN))
                    .await
                    .unwrap();
                gate.entered().await;
                if stop_owner {
                    archives.shutdown().await;
                } else {
                    service.shutdown().await;
                }
                gate.release();
                assert_eq!(
                    handle.wait().await.err(),
                    Some(DiscordImportError::StorageFailure),
                    "{point:?}, stop_owner={stop_owner}"
                );
                assert_eq!(handle.latest_progress().phase, DiscordImportPhase::Failed);
                assert_eq!(handle.latest_progress().parsed_records, 0);
                assert!(handle.checkpoint().is_none());
                assert_eq!(calls.load(Ordering::Acquire), 1);
                imports.shutdown().await;
                archives.shutdown().await;
            });
        }
    }
}

#[test]
fn discord_import_pre_registration_user_cancellation_does_not_open_storage() {
    for point in [
        TestPoint::Poll(DiscordImportPhase::Inspecting),
        TestPoint::HashChunk,
    ] {
        runtime().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("content.db");
            let archives = archive(&path);
            let (imports, gate, _release) = paused(archives.clone(), point);
            let handle = imports
                .start(selected(&dir.path().join("selected.zip"), FROZEN))
                .await
                .unwrap();
            gate.entered().await;
            handle.cancel();
            gate.release();
            assert_eq!(
                handle.wait().await.err(),
                Some(DiscordImportError::Cancelled)
            );
            assert_eq!(
                handle.latest_progress().phase,
                DiscordImportPhase::Cancelled
            );
            assert_eq!(handle.latest_progress().parsed_records, 0);
            assert!(handle.checkpoint().is_none());
            imports.shutdown().await;
            archives.shutdown().await;
            assert!(!path.exists());
        });
    }
}

#[test]
fn discord_import_lost_storage_reports_storage_failure_and_reopens_interrupted() {
    runtime().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content.db");
        let selected_path = directory.path().join("selected.zip");
        let archives = archive(&path);
        let (imports, gate, _release) = paused(archives.clone(), TestPoint::BeforeFinish);
        let handle = imports
            .start(selected(&selected_path, FROZEN))
            .await
            .unwrap();
        gate.entered().await;
        archives.open().await.unwrap().shutdown().await;
        gate.release();
        assert_eq!(
            handle.wait().await.err(),
            Some(DiscordImportError::StorageFailure)
        );
        imports.shutdown().await;
        archives.shutdown().await;
        let reopened = archive(&path);
        let imports = DiscordImportOwner::new(reopened.clone());
        let result = imports
            .start(File::open(&selected_path).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.disposition, ImportDisposition::RetryRequired);
        assert_eq!(result.checkpoint.progress.phase, ImportPhase::Interrupted);
        imports.shutdown().await;
        reopened.shutdown().await;
    });
}

#[test]
fn discord_import_parser_failure_after_acknowledgement_stays_hidden_and_retry_is_explicit() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let archives = archive(&dir.path().join("content.db"));
        let imports = DiscordImportOwner::new(archives.clone());
        let file = dir.path().join("selected.zip");
        let handle = imports
            .start(selected(&file, &package(700, "hidden@example.test", true)))
            .await
            .unwrap();
        assert_eq!(
            handle.wait().await.err(),
            Some(DiscordImportError::InvalidArchive)
        );
        let checkpoint = handle.checkpoint().unwrap();
        assert_eq!(checkpoint.progress.phase, ImportPhase::Failed);
        assert_eq!(checkpoint.progress.committed_items, 498);
        assert_eq!(checkpoint.progress.next_batch, 1);
        let again = imports.start(File::open(&file).unwrap()).await.unwrap();
        let resolution = again.wait().await.unwrap();
        assert_eq!(resolution.disposition, ImportDisposition::RetryRequired);
        assert_eq!(again.latest_progress().parsed_records, 0);
        assert_eq!(resolution.checkpoint, checkpoint);
        let retry = imports
            .retry(File::open(&file).unwrap(), &resolution)
            .await
            .unwrap();
        assert_eq!(
            retry.wait().await.err(),
            Some(DiscordImportError::InvalidArchive)
        );
        let retried = retry.checkpoint().unwrap();
        assert_eq!(retried.progress, checkpoint.progress);
        assert_eq!(retried.warnings, checkpoint.warnings);
        assert_eq!(retried.observed_at, checkpoint.observed_at);
        assert_eq!(retried.scope, checkpoint.scope);
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

#[test]
fn discord_import_exact_replay_preserves_digests_observation_time_and_rejects_changed_authority() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("selected.zip");
        let archives = archive(&dir.path().join("content.db"));
        let (imports, gate, _release) =
            paused_after(archives.clone(), TestPoint::BatchCommitted, 2);
        let handle = imports
            .start(selected(
                &file,
                &package(1300, "replayed@example.test", false),
            ))
            .await
            .unwrap();
        gate.entered().await;
        handle.cancel();
        gate.release();
        assert_eq!(
            handle.wait().await.err(),
            Some(DiscordImportError::Cancelled)
        );
        let checkpoint = handle.checkpoint().unwrap();
        assert_eq!(checkpoint.progress.committed_items, 998);
        let needed = imports
            .start(File::open(&file).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(needed.disposition, ImportDisposition::RetryRequired);
        for variant in 0..5 {
            let mut changed = needed.clone();
            match variant {
                0 => changed.checkpoint.schema_profile.version += 1,
                1 => changed.parser_policy = "discord.import.changed".into(),
                2 => changed.checkpoint.scope.account_id = uuid::Uuid::new_v4().try_into().unwrap(),
                3 => changed.checkpoint.scope.provider = "foreign".to_owned().try_into().unwrap(),
                _ => changed.checkpoint.scope.source_id = uuid::Uuid::new_v4().try_into().unwrap(),
            }
            let rejected = imports
                .retry(File::open(&file).unwrap(), &changed)
                .await
                .unwrap();
            assert_eq!(
                rejected.wait().await.err(),
                Some(DiscordImportError::RetryMismatch)
            );
        }
        let wrong_file = selected(&dir.path().join("other.zip"), FROZEN);
        assert_eq!(
            imports
                .retry(wrong_file, &needed)
                .await
                .unwrap()
                .wait()
                .await
                .err(),
            Some(DiscordImportError::RetryMismatch)
        );
        let retry = imports
            .retry(File::open(&file).unwrap(), &needed)
            .await
            .unwrap();
        let ready = wait_monotonic(&retry).await;
        assert_eq!(ready.checkpoint.scope, checkpoint.scope);
        assert_eq!(ready.checkpoint.observed_at, checkpoint.observed_at);
        assert_eq!(ready.checkpoint.progress.committed_items, 1300);
        assert_eq!(ready.checkpoint.progress.next_batch, 3);
        assert_eq!(ready.checkpoint.warnings, checkpoint.warnings);
        let service = archives.open().await.unwrap();
        assert!(
            query(&service, &ready)
                .await
                .iter()
                .all(|r| r.observed_at == checkpoint.observed_at)
        );
        // Cancellation after the readiness transaction cannot roll it back.
        retry.cancel();
        assert_eq!(retry.wait().await.unwrap().checkpoint, ready.checkpoint);
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

async fn wait_monotonic(handle: &super::import::DiscordImportHandle) -> DiscordImportOutcome {
    let mut previous = handle.latest_progress();
    let waiting = handle.wait();
    futures_util::pin_mut!(waiting);
    loop {
        let result = futures_util::poll!(waiting.as_mut());
        let next = handle.latest_progress();
        for (old, new) in [
            (previous.hashed_bytes, next.hashed_bytes),
            (previous.read_bytes, next.read_bytes),
            (previous.parsed_records, next.parsed_records),
            (previous.processed_entries, next.processed_entries),
            (previous.committed_items, next.committed_items),
            (previous.committed_bytes, next.committed_bytes),
            (previous.committed_batches, next.committed_batches),
            (previous.warnings, next.warnings),
        ] {
            assert!(new >= old);
        }
        previous = next;
        if let std::task::Poll::Ready(result) = result {
            return result.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

#[test]
fn discord_import_replaced_or_modified_retained_input_never_becomes_ready() {
    for replacement in [false, true] {
        runtime().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("selected.zip");
            let archives = archive(&dir.path().join("content.db"));
            let (imports, gate, _release) = paused(archives.clone(), TestPoint::BeforeVerification);
            let handle = imports.start(selected(&path, FROZEN)).await.unwrap();
            gate.entered().await;
            if replacement {
                fs::write(dir.path().join("replacement.zip"), FROZEN).unwrap();
                fs::rename(dir.path().join("replacement.zip"), &path).unwrap();
            } else {
                fs::write(&path, package(2, "changed", false)).unwrap();
            }
            gate.release();
            assert_eq!(
                handle.wait().await.err(),
                Some(DiscordImportError::InputChanged)
            );
            let checkpoint = handle.checkpoint().unwrap();
            assert_eq!(checkpoint.progress.phase, ImportPhase::Failed);
            assert_eq!(
                checkpoint.failure_code,
                Some(crate::persistence::archive::ImportFailureCode::InputChanged)
            );
            imports.shutdown().await;
            archives.shutdown().await;
        });
    }
}

#[test]
fn discord_import_owner_keeps_dropped_handle_and_waiter_and_rejects_active_identical_race() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let archives = archive(&dir.path().join("content.db"));
        let (imports, gate, _release) = paused(archives.clone(), TestPoint::BatchCommitted);
        let file = dir.path().join("selected.zip");
        let handle = imports.start(selected(&file, FROZEN)).await.unwrap();
        gate.entered().await;
        let mut waiter = Box::pin(handle.wait());
        assert!(futures_util::poll!(waiter.as_mut()).is_pending());
        drop(waiter);
        assert_eq!(
            imports.start(File::open(&file).unwrap()).await.err(),
            Some(DiscordImportError::Busy)
        );
        drop(handle);
        let mut shutdown = Box::pin(imports.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        gate.release();
        imports.shutdown().await;
        assert_eq!(
            imports.start(File::open(&file).unwrap()).await.err(),
            Some(DiscordImportError::Closed)
        );
        archives.shutdown().await;
        let archives = archive(&dir.path().join("content.db"));
        let imports = DiscordImportOwner::new(archives.clone());
        let result = imports
            .start(File::open(&file).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.disposition, ImportDisposition::RetryRequired);
        assert_eq!(result.checkpoint.progress.phase, ImportPhase::Cancelled);
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

#[test]
fn discord_import_unsupported_and_special_inputs_do_not_open_keys_and_failures_are_sticky() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let attempts = calls.clone();
        let archives = Arc::new(ArchiveOwner::with_opener(move || {
            attempts.fetch_add(1, Ordering::AcqRel);
            Err(ArchiveError::UnavailableKey)
        }));
        let imports = DiscordImportOwner::new(archives.clone());
        for file in [
            selected(&dir.path().join("bad.zip"), b"invalid input"),
            File::open("/dev/null").unwrap(),
        ] {
            assert_eq!(
                imports.start(file).await.unwrap().wait().await.err(),
                Some(DiscordImportError::InvalidArchive)
            );
        }
        assert_eq!(calls.load(Ordering::Acquire), 0);
        for n in 0..2 {
            assert_eq!(
                imports
                    .start(selected(&dir.path().join(format!("valid{n}.zip")), FROZEN))
                    .await
                    .unwrap()
                    .wait()
                    .await
                    .err(),
                Some(DiscordImportError::UnavailableKey)
            );
        }
        assert_eq!(calls.load(Ordering::Acquire), 1);
        imports.shutdown().await;
        archives.shutdown().await;
    });
}

#[test]
fn discord_import_retry_after_source_removal_does_not_register_a_replacement() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("selected.zip");
        let archives = archive(&dir.path().join("content.db"));
        let (imports, gate, _release) = paused(archives.clone(), TestPoint::BatchCommitted);
        let handle = imports.start(selected(&file, FROZEN)).await.unwrap();
        gate.entered().await;
        handle.cancel();
        gate.release();
        assert_eq!(
            handle.wait().await.err(),
            Some(DiscordImportError::Cancelled)
        );
        let expected = imports
            .start(File::open(&file).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        let service = archives.open().await.unwrap();
        service
            .remove_source(&expected.checkpoint.scope)
            .await
            .unwrap();
        let retry = imports
            .retry(File::open(&file).unwrap(), &expected)
            .await
            .unwrap();
        assert_eq!(
            retry.wait().await.err(),
            Some(DiscordImportError::RetryMismatch)
        );
        let fresh = imports
            .start(File::open(&file).unwrap())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(fresh.disposition, ImportDisposition::Ready);
        assert_ne!(
            fresh.checkpoint.scope.source_id,
            expected.checkpoint.scope.source_id
        );
        imports.shutdown().await;
        archives.shutdown().await;
    });
}
