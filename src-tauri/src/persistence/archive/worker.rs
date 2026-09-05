//! Bounded typed worker. SQLite, credential loading and destruction stay on its
//! blocking task. No command accepts SQL or deserializable mutation authority.
use super::{
    ArchiveError, ArchiveSearch, ArchiveStore, ImportBatch, ImportCheckpoint, ImportProgress,
    ImportSession, RemovalOutcome, model, query,
};
use crate::providers::ports::{ContentQuery, ConversationQuery, Page, QuerySource, ResolveRequest};
use async_trait::async_trait;
use retract_domain::{
    AccountRecord, ContentRecord, ConversationRecord, ProviderError, ProviderErrorKind, Scope,
    SourceRecord, VersionedPayload,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

type Reply<T> = oneshot::Sender<Result<T, ArchiveError>>;

#[cfg(test)]
pub(super) fn reserve_command_slot(service: &ArchiveService) -> impl Drop + '_ {
    service.sender.try_reserve().unwrap()
}

enum Command {
    Register(Box<(AccountRecord, SourceRecord)>, Reply<SourceRecord>),
    Source(Scope, Reply<SourceRecord>),
    Begin(Scope, Reply<Arc<ImportSession>>),
    Status(
        Scope,
        String,
        VersionedPayload,
        Reply<Option<ImportCheckpoint>>,
    ),
    Retry(Box<ImportCheckpoint>, Reply<Arc<ImportSession>>),
    Append(Arc<ImportSession>, u64, ImportBatch, Reply<ImportProgress>),
    Finish(Arc<ImportSession>, Reply<ImportProgress>),
    Cancel(Arc<ImportSession>, Reply<ImportProgress>),
    Search(ArchiveSearch, Reply<Page<ContentRecord>>),
    Conversations(ConversationQuery, Reply<Page<ConversationRecord>>),
    Resolve(ResolveRequest, Reply<Vec<ContentRecord>>),
    Remove(Scope, Reply<RemovalOutcome>),
    Cleanup(Scope, Reply<RemovalOutcome>),
}

pub(crate) struct ArchiveService {
    sender: mpsc::Sender<Command>,
    stopped: Arc<AtomicBool>,
    shutdown_signal: watch::Sender<bool>,
    ready: watch::Receiver<Option<Result<(), ArchiveError>>>,
    // Await through &mut: dropping a shutdown waiter retains the join handle.
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ArchiveService {
    /// Disposable injected setup only; the production owner registers start()
    /// before waiting, so cancelled open callers never orphan repository work.
    #[cfg(any(test, feature = "archive-bench"))]
    pub(super) async fn open(
        open: impl FnOnce() -> Result<ArchiveStore, ArchiveError> + Send + 'static,
    ) -> Result<Arc<Self>, ArchiveError> {
        let service = Self::start(open);
        service.wait_ready().await?;
        Ok(service)
    }

    pub(super) fn start(
        open: impl FnOnce() -> Result<ArchiveStore, ArchiveError> + Send + 'static,
    ) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel(model::MAX_QUEUED_BATCHES);
        let (ready_sender, ready) = watch::channel(None);
        let (shutdown_signal, mut shutdown_receiver) = watch::channel(false);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut store = match open() {
                Ok(store) => store,
                Err(error) => {
                    let _ = ready_sender.send(Some(Err(error)));
                    return;
                }
            };
            let _ = ready_sender.send(Some(Ok(())));
            let runtime = tokio::runtime::Handle::current();
            while let Some(command) = runtime.block_on(async {
                if *shutdown_receiver.borrow() {
                    return None;
                }
                // Reservations can exhaust command capacity without waking a
                // receiver. The independent signal must also wake this wait.
                let command = receiver.recv();
                let shutdown = shutdown_receiver.changed();
                futures_util::pin_mut!(command, shutdown);
                match futures_util::future::select(command, shutdown).await {
                    futures_util::future::Either::Left((command, _)) => command,
                    futures_util::future::Either::Right(_) => None,
                }
            }) {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match command {
                    Command::Register(records, reply) => {
                        let (account, source) = *records;
                        let _ = reply.send(store.register_source(account, source));
                    }
                    Command::Source(scope, reply) => {
                        let _ = reply.send(store.source(&scope));
                    }
                    Command::Begin(scope, reply) => {
                        let _ = reply.send(store.begin_import(&scope).map(Arc::new));
                    }
                    Command::Status(scope, fingerprint, schema, reply) => {
                        let _ = reply.send(store.import_status(&scope, &fingerprint, &schema));
                    }
                    Command::Retry(expected, reply) => {
                        let _ = reply.send(store.retry_import(&expected).map(Arc::new));
                    }
                    Command::Append(session, sequence, batch, reply) => {
                        let _ = reply.send(store.append_batch(&session, sequence, batch));
                    }
                    Command::Finish(session, reply) => {
                        let _ = reply.send(store.finish_import(&session));
                    }
                    Command::Cancel(session, reply) => {
                        let _ = reply.send(store.cancel_import(&session));
                    }
                    Command::Search(request, reply) => {
                        let _ = reply.send(store.search(request));
                    }
                    Command::Conversations(request, reply) => {
                        let _ = reply.send(store.list_conversations(request));
                    }
                    Command::Resolve(request, reply) => {
                        let _ = reply.send(store.resolve(request));
                    }
                    Command::Remove(scope, reply) => {
                        let _ = reply.send(store.remove_source(&scope));
                    }
                    Command::Cleanup(scope, reply) => {
                        let _ = reply.send(store.retry_cleanup(&scope));
                    }
                }
            }
            // Cancels response channels for all unstarted commands before exit.
            receiver.close();
            drop(receiver);
            drop(store);
        });
        Arc::new(Self {
            sender,
            stopped,
            shutdown_signal,
            ready,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub(super) async fn wait_ready(&self) -> Result<(), ArchiveError> {
        let mut ready = self.ready.clone();
        loop {
            self.check_running()?;
            if let Some(result) = *ready.borrow() {
                return result;
            }
            ready
                .changed()
                .await
                .map_err(|_| ArchiveError::StorageFailure)?;
        }
    }

    fn check_running(&self) -> Result<(), ArchiveError> {
        if self.stopped.load(Ordering::Acquire) {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<T, ArchiveError> {
        self.check_running()?;
        let permit = self
            .sender
            .reserve()
            .await
            .map_err(|_| ArchiveError::Cancelled)?;
        self.check_running()?;
        let (send, receive) = oneshot::channel();
        permit.send(command(send));
        receive.await.map_err(|_| ArchiveError::Cancelled)?
    }

    pub(crate) async fn register_source(
        &self,
        account: &AccountRecord,
        source: &SourceRecord,
    ) -> Result<SourceRecord, ArchiveError> {
        model::registration_bounds(account, source)?;
        self.request(|reply| Command::Register(Box::new((account.clone(), source.clone())), reply))
            .await
    }
    pub(crate) async fn source(&self, scope: &Scope) -> Result<SourceRecord, ArchiveError> {
        self.request(|reply| Command::Source(scope.clone(), reply))
            .await
    }
    pub(crate) async fn begin_import(
        &self,
        scope: &Scope,
    ) -> Result<Arc<ImportSession>, ArchiveError> {
        self.request(|reply| Command::Begin(scope.clone(), reply))
            .await
    }
    pub(crate) async fn import_status(
        &self,
        scope: &Scope,
        fingerprint: &str,
        schema: &VersionedPayload,
    ) -> Result<Option<ImportCheckpoint>, ArchiveError> {
        model::provenance_bounds(fingerprint, schema)?;
        self.request(|reply| {
            Command::Status(scope.clone(), fingerprint.into(), schema.clone(), reply)
        })
        .await
    }
    pub(crate) async fn retry_import(
        &self,
        expected: &ImportCheckpoint,
    ) -> Result<Arc<ImportSession>, ArchiveError> {
        model::checkpoint_bounds(expected)?;
        self.request(|reply| Command::Retry(Box::new(expected.clone()), reply))
            .await
    }

    /// Reserve before cloning. Even concurrent callers can retain only two
    /// accepted pending batches; Busy means retry with the same borrowed batch.
    pub(crate) fn append_batch(
        &self,
        session: &Arc<ImportSession>,
        sequence: u64,
        batch: &ImportBatch,
    ) -> Result<oneshot::Receiver<Result<ImportProgress, ArchiveError>>, ArchiveError> {
        self.check_running()?;
        batch.bounded_size()?;
        session.cancellation_signal().check()?;
        let slot = match self.sender.try_reserve() {
            Ok(slot) => slot,
            Err(mpsc::error::TrySendError::Full(_)) => return Err(ArchiveError::Busy),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(ArchiveError::Cancelled),
        };
        self.check_running()?;
        session.cancellation_signal().check()?;
        let (reply, result) = oneshot::channel();
        slot.send(Command::Append(
            session.clone(),
            sequence,
            batch.clone(),
            reply,
        ));
        Ok(result)
    }

    pub(crate) async fn finish_import(
        &self,
        session: &Arc<ImportSession>,
    ) -> Result<ImportProgress, ArchiveError> {
        self.request(|reply| Command::Finish(session.clone(), reply))
            .await
    }
    pub(crate) async fn cancel_import(
        &self,
        session: &Arc<ImportSession>,
    ) -> Result<ImportProgress, ArchiveError> {
        let cancellation = session.cancellation_signal();
        cancellation.cancel();
        let authority = Arc::clone(session);
        self.request(move |reply| Command::Cancel(authority, reply))
            .await
    }
    pub(crate) async fn search(
        &self,
        request: &ArchiveSearch,
    ) -> Result<Page<ContentRecord>, ArchiveError> {
        query::search_bounds(request)?;
        self.request(|reply| Command::Search(request.clone(), reply))
            .await
    }
    pub(crate) async fn list_conversations(
        &self,
        request: &ConversationQuery,
    ) -> Result<Page<ConversationRecord>, ArchiveError> {
        query::conversation_bounds(request)?;
        self.request(|reply| Command::Conversations(request.clone(), reply))
            .await
    }
    pub(crate) async fn resolve(
        &self,
        request: &ResolveRequest,
    ) -> Result<Vec<ContentRecord>, ArchiveError> {
        query::resolve_bounds(request)?;
        self.request(|reply| Command::Resolve(request.clone(), reply))
            .await
    }
    pub(crate) async fn remove_source(
        &self,
        scope: &Scope,
    ) -> Result<RemovalOutcome, ArchiveError> {
        self.request(|reply| Command::Remove(scope.clone(), reply))
            .await
    }
    pub(crate) async fn retry_cleanup(
        &self,
        scope: &Scope,
    ) -> Result<RemovalOutcome, ArchiveError> {
        self.request(|reply| Command::Cleanup(scope.clone(), reply))
            .await
    }

    pub(crate) async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.shutdown_signal.send_replace(true);
        let mut completion = self.worker.lock().await;
        if let Some(running) = completion.as_mut() {
            // Keep ownership in the service while awaiting completion, so a
            // cancelled waiter cannot detach the repository/key/lock lifetime.
            let _ = running.await;
        }
        completion.take();
    }
}

impl Drop for ArchiveService {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.shutdown_signal.send_replace(true);
    }
}

pub(crate) struct ArchiveQuerySource(pub(crate) Arc<ArchiveService>);

fn provider_error(error: ArchiveError) -> ProviderError {
    ProviderError {
        code: match error {
            ArchiveError::Busy
            | ArchiveError::StoreInUse
            | ArchiveError::Cancelled
            | ArchiveError::StorageFailure
            | ArchiveError::CleanupPending => ProviderErrorKind::Transient,
            ArchiveError::UnsupportedCodec | ArchiveError::UnsupportedSchema => {
                ProviderErrorKind::UnsupportedSchema
            }
            ArchiveError::UnavailableKey => ProviderErrorKind::PermissionChanged,
            ArchiveError::ScopeMismatch => ProviderErrorKind::NotFound,
            _ => ProviderErrorKind::InvalidArchive,
        },
        retry_at: None,
    }
}

#[async_trait]
impl QuerySource for ArchiveQuerySource {
    async fn list_conversations(
        &self,
        request: ConversationQuery,
    ) -> Result<Page<ConversationRecord>, ProviderError> {
        self.0
            .list_conversations(&request)
            .await
            .map_err(provider_error)
    }
    async fn search(&self, request: ContentQuery) -> Result<Page<ContentRecord>, ProviderError> {
        self.0
            .search(&ArchiveSearch {
                scope: request.scope,
                text: request.query,
                kinds: vec![],
                author: None,
                before: None,
                after: None,
                cursor: request.cursor,
                limit: request.limit,
            })
            .await
            .map_err(provider_error)
    }
    async fn resolve(&self, request: ResolveRequest) -> Result<Vec<ContentRecord>, ProviderError> {
        self.0.resolve(&request).await.map_err(provider_error)
    }
}
