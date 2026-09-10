//! Opt-in Linux measurement using generated data and the real owned import path.
use super::{DiscordPayloadValidator, import::DiscordImportOwner, locators::discord_provider_key};
use crate::persistence::{
    ProviderPayloadValidator,
    archive::{
        ArchiveKey, ArchiveOwner, ArchiveSearch, ArchiveService, ArchiveStore, ImportDisposition,
    },
};
use retract_domain::{PrivacyKind, Scope};
use serde_json::json;
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, File},
    io::{Seek, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const BASE_ID: u64 = 1985931830091579392;
const MARKER: &str = "invented.discord.bench@example.invalid";

fn validate_directory(directory: &Path, uid: u32) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    let canonical = fs::canonicalize(directory)?;
    let temporary_root = fs::canonicalize(std::env::temp_dir())?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || canonical.as_os_str() != directory.as_os_str()
        || !canonical.starts_with(&temporary_root)
        || canonical == temporary_root
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
        || fs::read_dir(directory)?.next().is_some()
    {
        return Err(
            "benchmark requires an owned empty canonical private temporary directory".into(),
        );
    }
    Ok(())
}

// Four fixed stored ZIP entries. Both sizing/CRC and writing regenerate one
// record at a time; neither pass holds a transcript or the complete ZIP.
fn contents(index: u64) -> String {
    format!(
        "Invented Discord benchmark {index:06} {}",
        if index.is_multiple_of(100) {
            MARKER
        } else {
            "ordinary"
        }
    )
}
fn row(index: u64) -> String {
    format!(
        r#"{{"ID":{},"Timestamp":"2030-01-02 03:04:05","Contents":"{}","Attachments":""}}"#,
        BASE_ID + index,
        contents(index)
    )
}
fn payload(
    index: usize,
    records: u64,
    mut consume: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match index {
        0 => consume(br#"{"id":"9007199254741001","username":"invented_benchmark_owner"}"#),
        1 => consume(b"{}"),
        2 => consume(br#"{"id":"9007199254741101","type":"invented_kind"}"#),
        _ => {
            consume(b"[")?;
            for index in 0..records {
                if index != 0 {
                    consume(b",")?;
                }
                consume(row(index).as_bytes())?;
            }
            consume(b"]")
        }
    }
}
fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn generate(path: &Path, records: u64) -> Result<u64> {
    if ![10_000, 100_000].contains(&records) {
        return Err("unsupported synthetic corpus size".into());
    }
    let mut output = std::io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?,
    );
    let names = [
        "Account/user.json",
        "Messages/index.json",
        "Messages/c9007199254741101/channel.json",
        "Messages/c9007199254741101/messages.json",
    ];
    let mut directory = Vec::new();
    let mut decoded = 0;
    for (index, name) in names.iter().enumerate() {
        let mut crc = !0u32;
        let mut size = 0u32;
        payload(index, records, |bytes| {
            size += bytes.len() as u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
                }
            }
            Ok(())
        })?;
        crc = !crc;
        let offset: u32 = output.stream_position()?.try_into()?;
        let mut local = [0; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        put16(&mut local, 4, 20);
        put32(&mut local, 14, crc);
        put32(&mut local, 18, size);
        put32(&mut local, 22, size);
        put16(&mut local, 26, name.len() as u16);
        output.write_all(&local)?;
        output.write_all(name.as_bytes())?;
        payload(index, records, |bytes| output.write_all(bytes))?;
        let mut central = [0; 46];
        central[..4].copy_from_slice(b"PK\x01\x02");
        put16(&mut central, 4, 0x0314);
        put16(&mut central, 6, 20);
        put32(&mut central, 16, crc);
        put32(&mut central, 20, size);
        put32(&mut central, 24, size);
        put16(&mut central, 28, name.len() as u16);
        put32(&mut central, 38, 0o100600 << 16);
        put32(&mut central, 42, offset);
        directory.extend_from_slice(&central);
        directory.extend_from_slice(name.as_bytes());
        decoded += u64::from(size);
    }
    let offset: u32 = output.stream_position()?.try_into()?;
    output.write_all(&directory)?;
    let mut end = [0; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    put16(&mut end, 8, 4);
    put16(&mut end, 10, 4);
    put32(&mut end, 12, directory.len() as u32);
    put32(&mut end, 16, offset);
    output.write_all(&end)?;
    output.flush()?;
    Ok(decoded)
}

#[derive(Clone, Default, serde::Serialize)]
struct Disk {
    db: u64,
    sidecars: u64,
    temporary: u64,
    total: u64,
}
fn disk(root: &Path) -> std::io::Result<Disk> {
    fn bytes(path: &Path) -> std::io::Result<u64> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_file() {
            return Ok(metadata.len());
        }
        if !metadata.is_dir() {
            return Err(std::io::Error::other("unexpected synthetic disk entry"));
        }
        fs::read_dir(path)?.try_fold(0, |total, entry| Ok(total + bytes(&entry?.path())?))
    }
    let mut result = Disk::default();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let size = bytes(&entry.path())?;
        let name = entry.file_name();
        if name == "content.db" {
            result.db += size;
        } else if name.to_string_lossy().starts_with("content.db-") {
            result.sidecars += size;
        } else if name != "synthetic.zip" {
            result.temporary += size;
        }
        result.total += size;
    }
    Ok(result)
}
fn peak_rss() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kb| kb * 1024)
        })
}
struct Sampler {
    stop: Arc<AtomicBool>,
    high: Arc<Mutex<Disk>>,
    failed: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Sampler {
    fn start(root: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let high = Arc::new(Mutex::new(Disk::default()));
        let failed = Arc::new(AtomicBool::new(false));
        let (signal, samples, errors) = (stop.clone(), high.clone(), failed.clone());
        let handle = thread::spawn(move || {
            while !signal.load(Ordering::Acquire) {
                match disk(&root) {
                    Ok(now) => {
                        let mut high = samples.lock().unwrap();
                        high.db = high.db.max(now.db);
                        high.sidecars = high.sidecars.max(now.sidecars);
                        high.temporary = high.temporary.max(now.temporary);
                        high.total = high.total.max(now.total);
                    }
                    // A journal may disappear between listing and stat.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        errors.store(true, Ordering::Release);
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            high,
            failed,
            handle: Some(handle),
        }
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
fn owner(path: &Path) -> Arc<ArchiveOwner> {
    let path = path.to_owned();
    Arc::new(ArchiveOwner::with_opener(move || {
        ArchiveStore::open(
            path.clone(),
            ArchiveKey::new([0x86; 32]),
            BTreeMap::from([(
                discord_provider_key(),
                Arc::new(DiscordPayloadValidator) as Arc<dyn ProviderPayloadValidator>,
            )]),
        )
    }))
}
async fn verify(service: &ArchiveService, scope: &Scope, records: u64) -> Result<()> {
    let mut request = ArchiveSearch {
        scope: scope.clone(),
        text: String::new(),
        kinds: vec![],
        author: None,
        before: None,
        after: None,
        cursor: None,
        limit: 200,
    };
    let mut count = 0u64;
    let mut findings = 0u64;
    let mut previous = None;
    loop {
        let page = service.search(&request).await?;
        assert!(page.items.len() <= 200);
        for record in page.items {
            // SQL pagination orders equal timestamps by descending stable UUID.
            if let Some(previous) = previous {
                assert!(record.id.as_uuid() < &previous);
            }
            previous = Some(*record.id.as_uuid());
            let native = record.resource.locator_payload["messageId"]
                .as_str()
                .ok_or("missing synthetic message identity")?
                .parse::<u64>()?;
            let index = native
                .checked_sub(BASE_ID)
                .ok_or("unexpected synthetic identity")?;
            assert!(index < records);
            assert_eq!(record.searchable_text, contents(index));
            let email = record
                .privacy_findings
                .iter()
                .filter(|finding| **finding == PrivacyKind::EmailAddress)
                .count();
            assert_eq!(email, usize::from(index.is_multiple_of(100)));
            findings += email as u64;
            count += 1;
        }
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            break;
        }
    }
    assert_eq!(count, records);
    assert_eq!(findings, records / 100);
    request.text = MARKER.into();
    let mut hits = 0;
    loop {
        let page = service.search(&request).await?;
        hits += page.items.len() as u64;
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            break;
        }
    }
    assert_eq!(hits, records / 100);
    Ok(())
}
fn phase(root: &Path, records: u64, phase: &str, started: Instant) -> Result<()> {
    println!(
        "{}",
        json!({"records":records,"phase":phase,"elapsed_seconds":started.elapsed().as_secs_f64(),"os_peak_rss_bytes":peak_rss(),"disk_bytes":disk(root)?})
    );
    Ok(())
}
async fn run_corpus(root: &Path, records: u64) -> Result<()> {
    let sampler = Sampler::start(root.to_owned());
    let archive = root.join("synthetic.zip");
    let path = root.join("content.db");
    let start = Instant::now();
    let decoded = generate(&archive, records)?;
    let raw = fs::metadata(&archive)?.len();
    phase(root, records, "generate", start)?;
    let archives = owner(&path);
    let imports = DiscordImportOwner::new(archives.clone());
    let start = Instant::now();
    let handle = imports.start(File::open(&archive)?).await?;
    let outcome = handle.wait().await?;
    assert_eq!(outcome.checkpoint.progress.committed_items, records);
    assert_eq!(handle.latest_progress().parsed_records, records);
    assert_eq!(handle.latest_progress().hashed_bytes, 2 * raw);
    println!(
        "{}",
        json!({"phase":"input_accounting","records":records,"raw_zip_bytes":raw,"decoded_entry_bytes":decoded,"accepted_normalized_bytes":outcome.checkpoint.progress.committed_bytes,"committed_batches":outcome.checkpoint.progress.next_batch,"physical_parser_read_bytes":handle.latest_progress().read_bytes,"hashed_bytes":handle.latest_progress().hashed_bytes})
    );
    phase(root, records, "inspect_hash_import_verify", start)?;
    let service = archives.open().await?;
    let start = Instant::now();
    verify(&service, &outcome.checkpoint.scope, records).await?;
    phase(root, records, "paginate_findings_search", start)?;
    imports.shutdown().await;
    archives.shutdown().await;
    drop(service);
    drop(imports);
    drop(archives);
    let start = Instant::now();
    let archives = owner(&path);
    let imports = DiscordImportOwner::new(archives.clone());
    let handle = imports.start(File::open(&archive)?).await?;
    let repeated = handle.wait().await?;
    assert_eq!(repeated.disposition, ImportDisposition::Ready);
    assert_eq!(repeated.checkpoint, outcome.checkpoint);
    assert_eq!(handle.latest_progress().parsed_records, 0);
    let service = archives.open().await?;
    verify(&service, &repeated.checkpoint.scope, records).await?;
    phase(root, records, "reopen_repeat_paginate", start)?;
    let start = Instant::now();
    let removed = service.remove_source(&outcome.checkpoint.scope).await?;
    assert_eq!(removed.removed_items, records);
    assert!(!removed.maintenance_pending);
    assert!(service.source(&outcome.checkpoint.scope).await.is_err());
    imports.shutdown().await;
    archives.shutdown().await;
    drop(service);
    drop(imports);
    drop(archives);
    phase(root, records, "remove_shutdown", start)?;
    assert_eq!(fs::metadata(&archive)?.len(), raw);
    println!(
        "{}",
        json!({"phase":"corpus_complete","records":records,"sampled_disk_high_bytes":sampler.high.lock().unwrap().clone(),"sampling_interval_ms":10,"sampling_error":sampler.failed.load(Ordering::Acquire),"os_peak_rss_bytes":peak_rss()})
    );
    drop(sampler);
    // Only artifacts generated inside the already validated private directory.
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected synthetic artifact kind".into());
        }
        fs::remove_file(entry.path())?;
    }
    assert!(fs::read_dir(root)?.next().is_none());
    Ok(())
}

/// Fixed 10k then 100k synthetic corpora, Linux-only OS memory measurement.
/// The only input is an empty private temporary directory; no archive input.
pub fn run_discord_import_benchmark(directory: &Path) -> Result<()> {
    let uid = fs::metadata("/proc/self")?.uid();
    validate_directory(directory, uid)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        for records in [10_000, 100_000] {
            run_corpus(directory, records).await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    #[test]
    fn benchmark_disk_counts_nested_temporary_files_separately() {
        let root = tempfile::tempdir().unwrap();
        for (name, size) in [
            ("content.db", 10),
            ("content.db-journal", 5),
            ("synthetic.zip", 1000),
            ("temp", 3),
        ] {
            std::fs::write(root.path().join(name), vec![0; size]).unwrap();
        }
        std::fs::create_dir(root.path().join("staging")).unwrap();
        std::fs::write(root.path().join("staging/ciphertext"), [0; 7]).unwrap();
        let observed = disk(root.path()).unwrap();
        assert_eq!(
            (
                observed.db,
                observed.sidecars,
                observed.temporary,
                observed.total
            ),
            (10, 5, 10, 1025)
        );
    }

    #[test]
    fn benchmark_rejects_unsafe_directory_before_writing() {
        let parent = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(parent.path()).unwrap();
        let directory = root.join("empty");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = std::fs::metadata(&directory).unwrap().uid();
        validate_directory(&directory, uid).unwrap();
        assert!(validate_directory(&directory, uid.wrapping_add(1)).is_err());
        assert!(validate_directory(&root.join("empty/../empty"), uid).is_err());
        let link = root.join("alias");
        symlink(&directory, &link).unwrap();
        assert!(validate_directory(&link, uid).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(validate_directory(&directory, uid).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(directory.join("sentinel"), b"invented").unwrap();
        assert!(validate_directory(&directory, uid).is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn benchmark_generation_is_deterministic_and_exercises_real_encrypted_import() {
        let parent = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(parent.path()).unwrap();
        let first = root.join("first.zip");
        let second = root.join("second.zip");
        let bytes = generate(&first, 10_000).unwrap();
        assert_eq!(generate(&second, 10_000).unwrap(), bytes);
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(run_corpus(&root, 10_000)).unwrap();
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }
}
