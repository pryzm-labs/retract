//! Backend-only owned import. A selected descriptor is never reopened by path.
use super::{
    DiscordNormalizer,
    locators::{DiscordUserLocator, discord_provider_key},
    model::DiscordSourceProfile,
    progress::{DiscordImportPhase as Phase, DiscordImportProgress},
};
use crate::persistence::archive::{
    ArchiveError, ArchiveOwner, ArchiveService, ImportBatchV2, ImportCancellation,
    ImportCheckpoint, ImportDisposition, ImportFailureCode, ImportPhase, ImportProgress,
    ImportSession, ImportWarningCode, ImportWarningDelta, MAX_BATCH_BYTES, MAX_BATCH_RECORDS,
    NewArchiveImport, encoded_size,
};
use discord_archive::{
    ArchiveInventory, ArchiveLimits, Cancellation, ChannelContext, DiscordArchiveReader, DiscordId,
    DiscordProfile, EntryIntegrity, ExportAccount, RecordSink, SentMessage,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};

// This binds normalization, lexicographic channel order, count/byte boundaries,
// warning placement and replay-from-zero to one backend-owned policy.
const PARSER_POLICY: &str = "discord.coordinator.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DiscordImportError {
    #[error("invalid_archive")]
    InvalidArchive,
    #[error("unsupported_profile")]
    UnsupportedProfile,
    #[error("limit_exceeded")]
    LimitExceeded,
    #[error("input_changed")]
    InputChanged,
    #[error("retry_mismatch")]
    RetryMismatch,
    #[error("unavailable_key")]
    UnavailableKey,
    #[error("storage_failure")]
    StorageFailure,
    #[error("cancelled")]
    Cancelled,
    #[error("busy")]
    Busy,
    #[error("closed")]
    Closed,
}
impl From<discord_archive::ArchiveError> for DiscordImportError {
    fn from(value: discord_archive::ArchiveError) -> Self {
        match value {
            discord_archive::ArchiveError::Cancelled => Self::Cancelled,
            discord_archive::ArchiveError::LimitExceeded => Self::LimitExceeded,
            discord_archive::ArchiveError::UnsupportedProfile => Self::UnsupportedProfile,
            _ => Self::InvalidArchive,
        }
    }
}
impl From<ArchiveError> for DiscordImportError {
    fn from(value: ArchiveError) -> Self {
        match value {
            ArchiveError::Cancelled => Self::Cancelled,
            ArchiveError::UnavailableKey => Self::UnavailableKey,
            ArchiveError::LimitExceeded => Self::LimitExceeded,
            ArchiveError::InvalidRecord | ArchiveError::ConflictingObservation => {
                Self::InvalidArchive
            }
            _ => Self::StorageFailure,
        }
    }
}
impl DiscordImportError {
    fn before_session(error: ArchiveError) -> Self {
        // Open/registration/retry do not receive our cancellation signal. Their
        // Cancelled means stopped storage or a lost command/reply channel, not
        // user cancellation (which is checked separately through Control).
        match error {
            ArchiveError::Cancelled => Self::StorageFailure,
            _ => error.into(),
        }
    }
    fn failure(self) -> ImportFailureCode {
        match self {
            Self::InputChanged => ImportFailureCode::InputChanged,
            Self::UnsupportedProfile => ImportFailureCode::UnsupportedProfile,
            Self::LimitExceeded => ImportFailureCode::LimitExceeded,
            Self::InvalidArchive | Self::RetryMismatch => ImportFailureCode::InvalidArchive,
            _ => ImportFailureCode::StorageFailure,
        }
    }
}

/// Backend result/retry authority, never deserialized from an IPC request.
#[derive(Debug, Clone)]
pub(crate) struct DiscordImportOutcome {
    pub checkpoint: ImportCheckpoint,
    pub disposition: ImportDisposition,
    pub(super) parser_policy: String,
}
type Completion = Result<DiscordImportOutcome, DiscordImportError>;

struct Control {
    cancelled: AtomicBool,
    session: Mutex<Option<ImportCancellation>>,
    checkpoint: Mutex<Option<ImportCheckpoint>>,
    progress: watch::Sender<DiscordImportProgress>,
    completion: watch::Sender<Option<Completion>>,
    #[cfg(test)]
    limits: ArchiveLimits,
    #[cfg(test)]
    hook: Arc<dyn Fn(TestPoint) + Send + Sync>,
}
impl Control {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(signal) = &*self.session.lock().unwrap() {
            signal.cancel();
        }
    }
    fn bind(&self, session: &ImportSession) {
        let mut bound = self.session.lock().unwrap();
        let signal = session.cancellation_signal();
        if self.cancelled.load(Ordering::Acquire) {
            signal.cancel();
        }
        *bound = Some(signal);
    }
    fn check(&self) -> Result<(), DiscordImportError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(DiscordImportError::Cancelled)
        } else {
            Ok(())
        }
    }
    fn phase(&self, phase: Phase) {
        self.progress.send_modify(|p| p.phase = phase);
    }
    fn committed(&self, value: &ImportProgress) {
        self.progress.send_modify(|p| {
            p.committed_items = p.committed_items.max(value.committed_items);
            p.committed_bytes = p.committed_bytes.max(value.committed_bytes);
            p.committed_batches = p.committed_batches.max(value.next_batch);
        });
    }
    fn checkpoint(&self, value: ImportCheckpoint) {
        self.committed(&value.progress);
        self.progress.send_modify(|p| {
            p.warnings = p.warnings.max(value.warnings.iter().map(|w| w.count).sum())
        });
        *self.checkpoint.lock().unwrap() = Some(value);
    }
    #[cfg(test)]
    fn point(&self, point: TestPoint) {
        (self.hook)(point);
    }
}
impl Cancellation for Control {
    fn is_cancelled(&self) -> bool {
        #[cfg(test)]
        self.point(TestPoint::Poll(self.progress.borrow().phase));
        self.cancelled.load(Ordering::Acquire)
    }
}

pub(crate) struct DiscordImportHandle {
    control: Arc<Control>,
}
impl DiscordImportHandle {
    pub(crate) fn latest_progress(&self) -> DiscordImportProgress {
        self.control.progress.borrow().clone()
    }
    pub(crate) fn checkpoint(&self) -> Option<ImportCheckpoint> {
        self.control.checkpoint.lock().unwrap().clone()
    }
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
    pub(crate) async fn wait(&self) -> Completion {
        let mut receive = self.control.completion.subscribe();
        loop {
            if let Some(result) = receive.borrow().clone() {
                return result;
            }
            receive
                .changed()
                .await
                .map_err(|_| DiscordImportError::StorageFailure)?;
        }
    }
}
struct Job {
    control: Arc<Control>,
    worker: tokio::task::JoinHandle<()>,
}
pub(crate) struct DiscordImportOwner {
    archives: Arc<ArchiveOwner>,
    closed: AtomicBool,
    active: Mutex<Option<Arc<Control>>>,
    job: AsyncMutex<Option<Job>>,
    #[cfg(test)]
    limits: ArchiveLimits,
    #[cfg(test)]
    hook: Arc<dyn Fn(TestPoint) + Send + Sync>,
}
impl DiscordImportOwner {
    /// Construction performs no file, archive, credential or runtime operations.
    pub(crate) fn new(archives: Arc<ArchiveOwner>) -> Self {
        Self {
            archives,
            closed: AtomicBool::new(false),
            active: Mutex::new(None),
            job: AsyncMutex::new(None),
            #[cfg(test)]
            limits: ArchiveLimits::default(),
            #[cfg(test)]
            hook: Arc::new(|_| {}),
        }
    }
    #[cfg(test)]
    pub(super) fn with_limits(archives: Arc<ArchiveOwner>, limits: ArchiveLimits) -> Self {
        let mut owner = Self::new(archives);
        owner.limits = limits;
        owner
    }
    #[cfg(test)]
    pub(crate) fn with_hook(
        archives: Arc<ArchiveOwner>,
        hook: impl Fn(TestPoint) + Send + Sync + 'static,
    ) -> Self {
        let mut owner = Self::new(archives);
        owner.hook = Arc::new(hook);
        owner
    }
    /// The future path-opening adapter must open read-only with O_NOFOLLOW (or
    /// equivalent). File alone cannot reveal whether its caller followed a link.
    /// Here we validate regular-file metadata and retain precisely this handle.
    pub(super) async fn start(
        &self,
        file: File,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        self.launch(file, None, false).await
    }
    pub(crate) async fn start_or_retry(
        &self,
        file: File,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        self.launch(file, None, true).await
    }
    pub(super) async fn retry(
        &self,
        file: File,
        expected: &DiscordImportOutcome,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        self.launch(file, Some(expected.clone()), false).await
    }
    pub(crate) async fn retry_active(
        &self,
        file: File,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        let checkpoint = self
            .active_checkpoint()
            .filter(|checkpoint| {
                matches!(
                    checkpoint.progress.phase,
                    ImportPhase::Interrupted | ImportPhase::Cancelled | ImportPhase::Failed
                )
            })
            .ok_or(DiscordImportError::RetryMismatch)?;
        self.retry(
            file,
            &DiscordImportOutcome {
                checkpoint,
                disposition: ImportDisposition::RetryRequired,
                parser_policy: PARSER_POLICY.into(),
            },
        )
        .await
    }
    async fn launch(
        &self,
        file: File,
        retry: Option<DiscordImportOutcome>,
        retry_existing: bool,
    ) -> Result<DiscordImportHandle, DiscordImportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DiscordImportError::Closed);
        }
        let mut owned = self.job.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(DiscordImportError::Closed);
        }
        if let Some(job) = owned.as_mut() {
            if job.control.completion.borrow().is_none() {
                return Err(DiscordImportError::Busy);
            }
            // Retain the old join while awaiting it; a dropped start waiter cannot detach it.
            let _ = (&mut job.worker).await;
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(DiscordImportError::Closed);
        }
        let (progress, _) = watch::channel(DiscordImportProgress::default());
        let (completion, _) = watch::channel(None);
        let control = Arc::new(Control {
            cancelled: AtomicBool::new(false),
            session: Mutex::new(None),
            checkpoint: Mutex::new(None),
            progress,
            completion,
            #[cfg(test)]
            limits: self.limits,
            #[cfg(test)]
            hook: self.hook.clone(),
        });
        let task_control = control.clone();
        let archives = self.archives.clone();
        let (launch, ready) = oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            task_control.point(TestPoint::BeforeLaunch);
            if ready.blocking_recv().is_err() {
                return;
            }
            let result = coordinate(
                file,
                &task_control,
                &archives,
                retry.as_ref(),
                retry_existing,
            );
            if let Err(error) = result {
                task_control.phase(if error == DiscordImportError::Cancelled {
                    Phase::Cancelled
                } else {
                    Phase::Failed
                });
            }
            task_control.completion.send_replace(Some(result));
        });
        *owned = Some(Job {
            control: control.clone(),
            worker,
        });
        {
            let mut active = self.active.lock().unwrap();
            *active = Some(control.clone());
            if self.closed.load(Ordering::Acquire) {
                control.cancel();
            }
        }
        #[cfg(test)]
        control.point(TestPoint::Installed);
        let _ = launch.send(());
        Ok(DiscordImportHandle { control })
    }
    pub(crate) fn reject_new_starts(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(control) = &*self.active.lock().unwrap() {
            control.cancel();
        }
    }
    pub(crate) fn active_progress(&self) -> Option<DiscordImportProgress> {
        self.active.lock().ok().and_then(|active| {
            active
                .as_ref()
                .map(|control| control.progress.borrow().clone())
        })
    }
    pub(crate) fn active_checkpoint(&self) -> Option<ImportCheckpoint> {
        self.active.lock().ok().and_then(|active| {
            active
                .as_ref()
                .and_then(|control| control.checkpoint.lock().ok()?.clone())
        })
    }
    pub(crate) fn cancel_active(&self) {
        if let Ok(active) = self.active.lock()
            && let Some(control) = active.as_ref()
        {
            control.cancel();
        }
    }
    #[cfg(test)]
    pub(crate) fn is_drained(&self) -> bool {
        self.job.try_lock().is_ok_and(|job| job.is_none())
    }
    pub(crate) async fn shutdown(&self) {
        self.reject_new_starts();
        let mut owned = self.job.lock().await;
        if let Some(job) = owned.as_mut() {
            job.control.cancel();
            // The owner retains this join even if the shutdown future is dropped.
            let _ = (&mut job.worker).await;
        }
        owned.take();
        self.active.lock().unwrap().take();
    }
}
impl Drop for DiscordImportOwner {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        if let Some(job) = self.job.get_mut() {
            job.control.cancel();
        }
    }
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    inode: u64,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl FileIdentity {
    fn read(file: &File) -> Result<Self, DiscordImportError> {
        let m = file
            .metadata()
            .map_err(|_| DiscordImportError::InvalidArchive)?;
        if !m.is_file() || m.nlink() == 0 {
            return Err(DiscordImportError::InvalidArchive);
        }
        Ok(Self {
            dev: m.dev(),
            inode: m.ino(),
            links: m.nlink(),
            size: m.size(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
    fn verify(&self, file: &File) -> Result<(), DiscordImportError> {
        if Self::read(file).as_ref() != Ok(self) {
            Err(DiscordImportError::InputChanged)
        } else {
            Ok(())
        }
    }
}
struct CountedFile<'a> {
    file: &'a Mutex<File>,
    control: &'a Control,
}
impl Read for CountedFile<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        #[cfg(test)]
        self.control.point(TestPoint::BeforeRead);
        // The inventory/reader owns cancellation polling and its typed error.
        // Returning an I/O error here would erase that classification in ZIP
        // preflight when cancellation races the parser's preceding poll.
        let n = self.file.lock().unwrap().read(bytes)?;
        self.control
            .progress
            .send_modify(|p| p.read_bytes += n as u64);
        Ok(n)
    }
}
impl Seek for CountedFile<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.lock().unwrap().seek(position)
    }
}
fn hash(
    selected: &Mutex<File>,
    control: &Control,
    size: u64,
    verifying: bool,
) -> Result<String, DiscordImportError> {
    // No entry reader is alive during hashing. Serialize access to the original
    // descriptor and restore its position before the retained inventory resumes.
    let mut file = selected.lock().unwrap();
    let position = file
        .stream_position()
        .map_err(|_| DiscordImportError::InputChanged)?;
    let result = hash_bytes(&mut file, control, size, verifying);
    let restored = file
        .seek(SeekFrom::Start(position))
        .map_err(|_| DiscordImportError::InputChanged);
    let digest = result?;
    restored?;
    Ok(digest)
}
fn hash_bytes(
    file: &mut File,
    control: &Control,
    size: u64,
    verifying: bool,
) -> Result<String, DiscordImportError> {
    control.check()?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| DiscordImportError::InputChanged)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        #[cfg(test)]
        control.point(if verifying {
            TestPoint::VerifyHashChunk
        } else {
            TestPoint::HashChunk
        });
        #[cfg(not(test))]
        let _ = verifying;
        control.check()?;
        let n = file
            .read(&mut bytes)
            .map_err(|_| DiscordImportError::InputChanged)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or(DiscordImportError::LimitExceeded)?;
        if total > size {
            return Err(DiscordImportError::InputChanged);
        }
        hash.update(&bytes[..n]);
        control.progress.send_modify(|p| p.hashed_bytes += n as u64);
    }
    control.check()?;
    if total != size {
        return Err(DiscordImportError::InputChanged);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
struct Live {
    service: Arc<ArchiveService>,
    session: Arc<ImportSession>,
    checkpoint: ImportCheckpoint,
}
fn status(live: &Live) -> Result<ImportCheckpoint, DiscordImportError> {
    tokio::runtime::Handle::current()
        .block_on(live.service.import_status(
            &live.checkpoint.scope,
            &live.checkpoint.fingerprint,
            &live.checkpoint.schema_profile,
        ))
        .map_err(DiscordImportError::from)?
        .ok_or(DiscordImportError::StorageFailure)
}
fn outcome(
    checkpoint: ImportCheckpoint,
    disposition: ImportDisposition,
    control: &Control,
) -> DiscordImportOutcome {
    control.phase(match checkpoint.progress.phase {
        ImportPhase::Ready => Phase::Ready,
        ImportPhase::Cancelled => Phase::Cancelled,
        ImportPhase::Failed | ImportPhase::Interrupted => Phase::Failed,
        ImportPhase::Importing => Phase::Importing,
    });
    control.checkpoint(checkpoint.clone());
    DiscordImportOutcome {
        checkpoint,
        disposition,
        parser_policy: PARSER_POLICY.into(),
    }
}
fn coordinate(
    file: File,
    control: &Control,
    archives: &ArchiveOwner,
    retry: Option<&DiscordImportOutcome>,
    retry_existing: bool,
) -> Completion {
    let file = Mutex::new(file);
    let mut live = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&file, control, archives, retry, retry_existing, &mut live)
    }))
    .unwrap_or(Err(DiscordImportError::StorageFailure));
    match (result, live) {
        (Err(error), Some(live)) => {
            let runtime = tokio::runtime::Handle::current();
            let transition = if error == DiscordImportError::Cancelled {
                runtime.block_on(live.service.cancel_import(&live.session))
            } else {
                runtime.block_on(live.service.fail_import(&live.session, error.failure()))
            };
            // If even the durable status cannot be read, cancellation/failure
            // persistence is unknown. Report unavailable storage; reopen will
            // classify the unfinished run as interrupted, never as Ready.
            let checkpoint = status(&live).map_err(|_| DiscordImportError::StorageFailure)?;
            if checkpoint.progress.phase == ImportPhase::Ready {
                return Ok(outcome(checkpoint, ImportDisposition::Ready, control));
            }
            control.checkpoint(checkpoint);
            if transition.is_err()
                && !matches!(
                    transition,
                    Err(ArchiveError::StaleCursor | ArchiveError::Cancelled)
                )
            {
                return Err(DiscordImportError::StorageFailure);
            }
            Err(error)
        }
        (result, _) => result,
    }
}
fn run(
    file: &Mutex<File>,
    control: &Control,
    archives: &ArchiveOwner,
    retry: Option<&DiscordImportOutcome>,
    retry_existing: bool,
    live: &mut Option<Live>,
) -> Completion {
    control.check()?;
    let identity = FileIdentity::read(&file.lock().unwrap())?;
    #[cfg(not(test))]
    let limits = ArchiveLimits::default();
    #[cfg(test)]
    let limits = control.limits;
    if identity.size > limits.max_archive_bytes {
        return Err(DiscordImportError::LimitExceeded);
    }
    control
        .progress
        .send_modify(|p| p.total_bytes = Some(identity.size));
    let mut inventory = ArchiveInventory::inspect(CountedFile { file, control }, limits, control)?;
    control
        .progress
        .send_modify(|p| p.inventory_entries = Some(inventory.entry_count() as u64));
    let profile = DiscordProfile::detect(&mut inventory)?;
    identity.verify(&file.lock().unwrap())?;
    control.phase(Phase::Hashing);
    let fingerprint = hash(file, control, identity.size, false)?;
    identity.verify(&file.lock().unwrap())?;
    let account = ExportAccount {
        id: DiscordId::parse(&profile.account.id)?,
        username: profile.account.username.clone(),
    };
    if let Some(expected) = retry
        && (expected.parser_policy != PARSER_POLICY
            || expected.disposition != ImportDisposition::RetryRequired
            || expected.checkpoint.fingerprint != fingerprint
            || expected.checkpoint.schema_profile != DiscordSourceProfile::payload()
            || expected.checkpoint.scope.provider != discord_provider_key())
    {
        return Err(DiscordImportError::RetryMismatch);
    }
    control.check()?;
    control.phase(Phase::Registering);
    let runtime = tokio::runtime::Handle::current();
    let service = runtime
        .block_on(archives.open())
        .map_err(DiscordImportError::before_session)?;
    control.check()?;
    let (checkpoint, session) = if let Some(expected) = retry {
        // Retry never calls get-or-register: a removed/stale scope must not
        // create another source. The store checks the complete checkpoint and
        // current provider policy transactionally. The full hash above binds
        // the parsed account to the backend result's original source identity.
        let session = runtime
            .block_on(service.retry_import(&expected.checkpoint))
            .map_err(|error| match error {
                ArchiveError::ScopeMismatch
                | ArchiveError::StaleCursor
                | ArchiveError::InvalidRecord
                | ArchiveError::ConflictingObservation
                | ArchiveError::LimitExceeded => DiscordImportError::RetryMismatch,
                _ => DiscordImportError::before_session(error),
            })?;
        (expected.checkpoint.clone(), session)
    } else {
        let resolution = runtime
            .block_on(
                service.resolve_or_register_import(NewArchiveImport {
                    provider: discord_provider_key(),
                    native_identity: DiscordUserLocator::new(account.id.as_str())
                        .map_err(|_| DiscordImportError::InvalidArchive)?
                        .payload(),
                    display_name: account.username.clone(),
                    username: Some(account.username.clone()),
                    avatar: None,
                    fingerprint,
                    schema_profile: DiscordSourceProfile::payload(),
                    parser_policy: PARSER_POLICY.into(),
                    observed_at: chrono::Utc::now(),
                }),
            )
            .map_err(DiscordImportError::before_session)?;
        if resolution.disposition == ImportDisposition::RetryRequired && retry_existing {
            let session = runtime
                .block_on(service.retry_import(&resolution.checkpoint))
                .map_err(|error| match error {
                    ArchiveError::ScopeMismatch
                    | ArchiveError::StaleCursor
                    | ArchiveError::InvalidRecord
                    | ArchiveError::ConflictingObservation
                    | ArchiveError::LimitExceeded => DiscordImportError::RetryMismatch,
                    _ => DiscordImportError::before_session(error),
                })?;
            (resolution.checkpoint, session)
        } else if resolution.disposition != ImportDisposition::Start {
            return Ok(outcome(
                resolution.checkpoint,
                resolution.disposition,
                control,
            ));
        } else {
            (
                resolution.checkpoint,
                resolution
                    .session
                    .ok_or(DiscordImportError::StorageFailure)?,
            )
        }
    };
    control.checkpoint(checkpoint.clone());
    control.bind(&session);
    *live = Some(Live {
        service: service.clone(),
        session: session.clone(),
        checkpoint: checkpoint.clone(),
    });
    #[cfg(test)]
    control.point(TestPoint::Registered);
    control.check()?;
    control.phase(Phase::Importing);
    let normalizer = DiscordNormalizer::new(checkpoint.scope.clone(), checkpoint.observed_at)
        .map_err(|_| DiscordImportError::InvalidArchive)?;
    let mut sink = Sink {
        normalizer,
        account,
        channel: None,
        control,
        service,
        session,
        batch: ImportBatchV2::default(),
        count: 0,
        bytes: 0,
        sequence: 0,
        failure: None,
    };
    sink.batch.records.actors.push(
        sink.normalizer
            .actor(&sink.account)
            .map_err(|_| DiscordImportError::InvalidArchive)?,
    );
    sink.count = 1;
    sink.bytes = encoded_size(&sink.batch.records.actors[0], MAX_BATCH_BYTES)?;
    {
        let mut reader = DiscordArchiveReader::open(inventory, profile, control)?;
        control.progress.send_modify(|p| p.processed_entries = 2);
        let summary = reader
            .visit_channels(&mut sink)
            .map_err(|e| sink.failure.unwrap_or_else(|| e.into()))?;
        control
            .progress
            .send_modify(|p| p.total_records = Some(summary.messages));
    }
    sink.flush()?;
    #[cfg(test)]
    control.point(TestPoint::BeforeVerification);
    control.check()?;
    control.phase(Phase::Verifying);
    identity.verify(&file.lock().unwrap())?;
    let fingerprint = hash(file, control, identity.size, true)?;
    identity.verify(&file.lock().unwrap())?;
    let live = live.as_ref().ok_or(DiscordImportError::StorageFailure)?;
    if fingerprint != live.checkpoint.fingerprint {
        return Err(DiscordImportError::InputChanged);
    }
    #[cfg(test)]
    control.point(TestPoint::BeforeFinish);
    control.check()?;
    // Check metadata again after any wait before finalizing; no path is reopened.
    identity.verify(&file.lock().unwrap())?;
    runtime.block_on(live.service.finish_import(&live.session))?;
    Ok(outcome(status(live)?, ImportDisposition::Ready, control))
}

struct Sink<'a> {
    normalizer: DiscordNormalizer,
    account: ExportAccount,
    channel: Option<ChannelContext>,
    control: &'a Control,
    service: Arc<ArchiveService>,
    session: Arc<ImportSession>,
    batch: ImportBatchV2,
    count: usize,
    bytes: usize,
    sequence: u64,
    failure: Option<DiscordImportError>,
}
impl Sink<'_> {
    fn room(&mut self, record: &impl serde::Serialize) -> Result<(), DiscordImportError> {
        self.control.check()?;
        let size = encoded_size(record, MAX_BATCH_BYTES)?;
        // Fixed conservative room for arrays, versioned digest framing and the
        // one closed warning code. This boundary is part of PARSER_POLICY.
        if self.count == MAX_BATCH_RECORDS || self.bytes + size + 1 > MAX_BATCH_BYTES - 1024 {
            self.flush()?;
        }
        if size > MAX_BATCH_BYTES - 1024 {
            return Err(DiscordImportError::LimitExceeded);
        }
        self.bytes += size + 1;
        self.count += 1;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DiscordImportError> {
        if self.count == 0 {
            return Ok(());
        }
        self.control.check()?;
        let runtime = tokio::runtime::Handle::current();
        let acknowledgement = loop {
            self.control.check()?;
            match self
                .service
                .append_batch_v2(&self.session, self.sequence, &self.batch)
            {
                Ok(reply) => break reply,
                Err(ArchiveError::Busy) => {
                    #[cfg(test)]
                    self.control.point(TestPoint::QueueBusy);
                    runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(2)));
                }
                Err(error) => return Err(error.into()),
            }
        };
        let progress = acknowledgement
            .blocking_recv()
            .map_err(|_| DiscordImportError::StorageFailure)??;
        self.control.committed(&progress);
        // Read warnings from the committed checkpoint, including replay counts.
        let live = Live {
            service: self.service.clone(),
            session: self.session.clone(),
            checkpoint: self
                .control
                .checkpoint
                .lock()
                .unwrap()
                .clone()
                .ok_or(DiscordImportError::StorageFailure)?,
        };
        self.control.checkpoint(status(&live)?);
        self.sequence += 1;
        self.batch = ImportBatchV2::default();
        self.count = 0;
        self.bytes = 0;
        #[cfg(test)]
        self.control.point(TestPoint::BatchCommitted);
        self.control.check()
    }
    fn error(
        &mut self,
        result: Result<(), DiscordImportError>,
    ) -> Result<(), discord_archive::ArchiveError> {
        result.map_err(|error| {
            self.failure = Some(error);
            if error == DiscordImportError::Cancelled {
                discord_archive::ArchiveError::Cancelled
            } else {
                discord_archive::ArchiveError::InvalidArchive
            }
        })
    }
}
impl RecordSink for Sink<'_> {
    fn begin_channel(
        &mut self,
        channel: ChannelContext,
    ) -> Result<(), discord_archive::ArchiveError> {
        let result = (|| {
            let record = self
                .normalizer
                .conversation(&channel)
                .map_err(|_| DiscordImportError::InvalidArchive)?;
            self.room(&record)?;
            self.batch.records.conversations.push(record);
            if let Some(warning) = self.batch.warnings.first_mut() {
                warning.count += 1;
            } else {
                self.batch.warnings.push(ImportWarningDelta {
                    code: ImportWarningCode::UnknownConversationKind,
                    count: 1,
                });
            }
            self.channel = Some(channel);
            Ok(())
        })();
        self.error(result)
    }
    fn message(&mut self, message: SentMessage) -> Result<(), discord_archive::ArchiveError> {
        let result = (|| {
            self.control.check()?;
            let channel = self
                .channel
                .as_ref()
                .ok_or(DiscordImportError::InvalidArchive)?;
            let record = self
                .normalizer
                .content(&self.account, channel, &message)
                .map_err(|_| DiscordImportError::InvalidArchive)?;
            self.room(&record)?;
            self.batch.records.contents.push(record);
            self.control.progress.send_modify(|p| p.parsed_records += 1);
            Ok(())
        })();
        self.error(result)
    }
    fn end_channel(&mut self, _: EntryIntegrity) -> Result<(), discord_archive::ArchiveError> {
        self.control
            .progress
            .send_modify(|p| p.processed_entries += 2);
        self.channel = None;
        self.error(self.control.check())
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestPoint {
    Installed,
    BeforeLaunch,
    BeforeRead,
    Registered,
    Poll(Phase),
    HashChunk,
    VerifyHashChunk,
    BatchCommitted,
    QueueBusy,
    BeforeVerification,
    BeforeFinish,
}
