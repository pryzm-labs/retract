//! Opt-in synthetic support, absent from normal builds. All storage work uses
//! the same repository, bounded worker and QuerySource used by the backend.
use super::{ArchiveKey, ArchiveQuerySource, ArchiveService, ArchiveStore, ImportBatch};
use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
    providers::ports::{ContentQuery, QuerySource},
};
use retract_domain::{
    AccountRecord, ActorRecord, ContentRecord, ConversationRecord, ProviderKey,
    ProviderResourceRef, RemediationPlan, ResourceKind, Scope, SourceRecord, VersionedPayload,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Synthetic;
fn invalid() -> AppError {
    AppError::StateUnavailable
}
fn provider() -> ProviderKey {
    "archive-benchmark".to_owned().try_into().unwrap()
}
impl ProviderPayloadValidator for Synthetic {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "archive-benchmark-v1".to_owned().try_into().unwrap()
    }
    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        if account.provider != provider()
            || account.native_identity.schema != "bench.account"
            || account.native_identity.version != 1
            || account.native_identity.payload != json!({"id": "1"})
            || account.avatar.is_some()
        {
            return Err(invalid());
        }
        "bench:1".to_owned().try_into()
    }
    fn validate_source(&self, source: &SourceRecord, _: &AccountRecord) -> Result<(), AppError> {
        if source.schema_profile != schema() {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        if resource.provider != provider()
            || resource.locator_schema != "bench.resource"
            || resource.locator_version != 1
            || resource.locator_payload != json!({"id": resource.canonical_key})
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_recipe(&self, _: &RemediationPlan) -> Result<(), AppError> {
        Err(invalid())
    }
    fn validate_archive_actor(&self, record: &ActorRecord) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        if record.avatar.is_some() {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_archive_conversation(&self, record: &ConversationRecord) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        if record.provider_metadata.is_some() || !record.participants.is_empty() {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_archive_content(&self, record: &ContentRecord) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        if record.provider_metadata.is_some() || !record.attachments.is_empty() {
            return Err(invalid());
        }
        Ok(())
    }
}
fn schema() -> VersionedPayload {
    VersionedPayload {
        schema: "bench.archive".into(),
        version: 1,
        payload: json!({"format": "synthetic-v1"}),
    }
}
fn account() -> AccountRecord {
    serde_json::from_value(json!({
    "id": "11111111-1111-4111-8111-111111111111", "provider": "archive-benchmark",
    "nativeIdentity": {"schema": "bench.account", "version": 1, "payload": {"id": "1"}},
    "displayName": "Synthetic benchmark", "username": null, "avatar": null, "connectionState": "disconnected",
    "createdAt": "2026-09-05T00:00:00Z", "lastSeenAt": "2026-09-05T00:00:00Z"
})).unwrap()
}
fn source(index: u32) -> SourceRecord {
    serde_json::from_value(json!({
    "id": if index == 0 { "22222222-2222-4222-8222-222222222222" } else { "33333333-3333-4333-8333-333333333333" },
    "accountId": "11111111-1111-4111-8111-111111111111", "provider": "archive-benchmark", "kind": "archive_import", "state": "preparing",
    "archiveFingerprint": format!("synthetic-export-{index}"), "schemaProfile": schema(), "importedAt": null,
    "updatedAt": "2026-09-05T00:00:00Z", "warnings": []
})).unwrap()
}
fn resource(native: &str, kind: ResourceKind) -> ProviderResourceRef {
    serde_json::from_value(json!({
    "provider": "archive-benchmark", "accountId": "11111111-1111-4111-8111-111111111111", "resourceKind": kind,
    "locatorSchema": "bench.resource", "locatorVersion": 1, "canonicalKey": native, "locatorPayload": {"id": native}
})).unwrap()
}
fn seed(scope: &Scope) -> ImportBatch {
    let actor_ref = resource("author", ResourceKind::Actor);
    let conversation_ref = resource("room", ResourceKind::Conversation);
    let actor: ActorRecord = serde_json::from_value(json!({"id": actor_ref.resource_id().unwrap(), "scope": scope, "resource": actor_ref,
        "displayName": "Synthetic actor", "username": null, "avatar": null, "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z"})).unwrap();
    let conversation: ConversationRecord = serde_json::from_value(json!({"id": conversation_ref.resource_id().unwrap(), "scope": scope, "resource": conversation_ref,
        "kind": "group", "title": "Synthetic room", "parentId": null, "participantCount": 1, "participants": [], "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z", "providerMetadata": null})).unwrap();
    let content_ref = resource("0", ResourceKind::Content);
    let content: ContentRecord = serde_json::from_value(json!({"id": content_ref.resource_id().unwrap(), "scope": scope, "resource": content_ref,
        "conversationId": conversation.id, "authorId": actor.id, "timestamp": "2026-09-05T00:00:00Z", "editedAt": null, "kind": "text", "searchableText": "",
        "attachments": [], "replyTo": null, "threadParent": null, "externalLocation": "unavailable", "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z",
        "privacyFindings": [], "detectorVersion": null, "providerMetadata": null})).unwrap();
    ImportBatch {
        actors: vec![actor],
        conversations: vec![conversation],
        contents: vec![content],
    }
}

#[derive(Default, Clone, serde::Serialize)]
struct Disk {
    db: u64,
    wal: u64,
    total: u64,
}
fn disk(root: &Path) -> Disk {
    fn total(root: &Path) -> u64 {
        fs::read_dir(root)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| match entry.file_type() {
                        Ok(kind) if kind.is_dir() => total(&entry.path()),
                        Ok(kind) if kind.is_file() => {
                            entry.metadata().map_or(0, |metadata| metadata.len())
                        }
                        _ => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    Disk {
        db: fs::metadata(root.join("content.db")).map_or(0, |m| m.len()),
        wal: fs::metadata(root.join("content.db-wal")).map_or(0, |m| m.len()),
        total: total(root),
    }
}
fn peak_rss() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        })
}
struct Sampler {
    stop: Arc<AtomicBool>,
    high: Arc<Mutex<Disk>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Sampler {
    fn start(root: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let high = Arc::new(Mutex::new(Disk::default()));
        let signal = stop.clone();
        let samples = high.clone();
        let thread = thread::spawn(move || {
            while !signal.load(Ordering::Acquire) {
                let current = disk(&root);
                let mut high = samples.lock().unwrap();
                high.db = high.db.max(current.db);
                high.wal = high.wal.max(current.wal);
                high.total = high.total.max(current.total);
                drop(high);
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            high,
            thread: Some(thread),
        }
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn count(
    query: &ArchiveQuerySource,
    scope: Scope,
    text: &str,
    start: u64,
    end: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut cursor = None;
    let mut count = 0;
    let mut previous = None;
    loop {
        let page = query
            .search(ContentQuery {
                scope: scope.clone(),
                query: text.into(),
                cursor,
                limit: 200,
            })
            .await
            .map_err(|_| "synthetic query failed")?;
        assert!(page.items.len() <= 200);
        for record in page.items {
            let native: u64 = record.resource.canonical_key.parse()?;
            assert!((start..end).contains(&native));
            if text == "needle" {
                assert_eq!(native % 100, 0);
            }
            let id = *record.id.as_uuid();
            if let Some(previous) = previous {
                assert!(id < previous);
            }
            previous = Some(id);
            count += 1;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(count);
        }
    }
}

/// Runs a fixed 100,000-item corpus in an exclusively owned empty disposable
/// directory. No credential, path, key, provider or corpus-size overrides exist.
pub fn run_archive_storage_benchmark(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = fs::canonicalize(directory)?;
    if root != directory
        || fs::metadata(&root)?.permissions().mode() & 0o077 != 0
        || fs::read_dir(&root)?.next().is_some()
    {
        return Err("benchmark requires an empty canonical private directory".into());
    }
    let _exclusive = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("benchmark.lock"))?;
    let sampler = Sampler::start(root.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        let path = root.join("content.db");
        let validators = BTreeMap::from([(provider(), Arc::new(Synthetic) as Arc<dyn ProviderPayloadValidator>)]);
        let service = ArchiveService::open(move || ArchiveStore::open(path, ArchiveKey::new([0x84; 32]), validators)).await?;
        let import_start = Instant::now();
        let mut committed_bytes = 0;
        for index in 0..2 {
            let source = source(index); let scope = source.scope();
            service.register_source(&account(), &source).await?;
            let session = service.begin_import(&scope).await?;
            let seed = seed(&scope); let mut next = u64::from(index) * 50_000;
            let end = next + 50_000; let mut sequence = 0;
            while next < end {
                let mut batch = ImportBatch { actors: seed.actors.clone(), conversations: seed.conversations.clone(), contents: Vec::with_capacity(498) };
                while next < end && batch.contents.len() < 498 {
                    let mut content = seed.contents[0].clone();
                    content.resource = resource(&next.to_string(), ResourceKind::Content); content.id = content.resource.resource_id()?.try_into()?;
                    content.searchable_text = format!("synthetic archive marker {} {next}", if next % 100 == 0 { "needle" } else { "ordinary" });
                    batch.contents.push(content); next += 1;
                }
                assert!(batch.bounded_size()? <= 4 * 1024 * 1024);
                let progress = service.append_batch(&session, sequence, &batch)?.await?;
                assert_eq!(progress?.next_batch, sequence + 1); sequence += 1;
            }
            let progress = service.finish_import(&session).await?;
            assert_eq!(progress.committed_items, 50_000); committed_bytes += progress.committed_bytes;
            println!("{}", json!({"phase":"source_committed", "source":index, "items":progress.committed_items, "bytes":progress.committed_bytes, "batches":progress.next_batch, "disk_bytes":disk(&root), "os_peak_rss_bytes":peak_rss()}));
        }
        println!("{}", json!({"phase":"import", "elapsed_seconds":import_start.elapsed().as_secs_f64(), "committed_items":100_000, "committed_bytes":committed_bytes, "disk_bytes":disk(&root), "os_peak_rss_bytes":peak_rss()}));
        let query = ArchiveQuerySource(service.clone()); let query_start = Instant::now();
        for index in 0..2 {
            let start = u64::from(index) * 50_000;
            assert_eq!(count(&query, source(index).scope(), "", start, start + 50_000).await?, 50_000);
            assert_eq!(count(&query, source(index).scope(), "needle", start, start + 50_000).await?, 500);
        }
        println!("{}", json!({"phase":"query", "elapsed_seconds":query_start.elapsed().as_secs_f64(), "total_items":100_000, "search_hits":1_000, "os_peak_rss_bytes":peak_rss()}));
        let removal_start = Instant::now(); let removed = service.remove_source(&source(0).scope()).await?;
        assert_eq!(removed.removed_items, 50_000); assert!(!removed.maintenance_pending);
        println!("{}", json!({"phase":"removal_with_50000_survivors", "elapsed_seconds":removal_start.elapsed().as_secs_f64(), "removed_items":removed.removed_items, "maintenance_pending":removed.maintenance_pending, "disk_bytes":disk(&root), "os_peak_rss_bytes":peak_rss()}));
        assert!(service.source(&source(0).scope()).await.is_err());
        assert_eq!(count(&query, source(1).scope(), "", 50_000, 100_000).await?, 50_000);
        assert_eq!(count(&query, source(1).scope(), "needle", 50_000, 100_000).await?, 500);
        service.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    println!(
        "{}",
        json!({"phase":"complete", "os_peak_rss_bytes":peak_rss(), "sampled_disk_high_bytes":sampler.high.lock().unwrap().clone(), "disk_bytes":disk(&root), "disk_sampling_interval_ms":10, "profile":"dev, debug info disabled by Docker"})
    );
    Ok(())
}
