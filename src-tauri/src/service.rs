use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use cleaner_domain::{ChatSummary, ConfirmationProof, DeletionPlan, DeletionReach, PlanOperation};
use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const DIRECT_CHAT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const DIRECT_CHAT_LOOKUP_CONCURRENCY: usize = 8;

use crate::{
    error::AppError,
    gateway::TelegramGateway,
    model::{
        AppSnapshot, AuthSnapshot, AuthorizePlanRequest, CatalogProgress, ExecuteRequest,
        JobRecord, JobStatus, MessageRef, PersistedState, PlanView, PrepareChatActionRequest,
        PrepareSelectionRequest, PrepareSenderActionRequest, SearchRequest, SearchResponse,
    },
    providers::telegram::engine_context::{
        EngineContext, LegacyTelegramRepository, SessionGateway, TelegramStateRepository,
    },
    secure_store::SecureJobStore,
};

pub struct CleanerService {
    gateway: Arc<dyn TelegramGateway>,
    plans: RwLock<HashMap<Uuid, DeletionPlan>>,
    jobs: RwLock<HashMap<Uuid, JobRecord>>,
    cancellation: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    system_grants: Mutex<HashMap<Uuid, SystemGrant>>,
    store: Arc<dyn TelegramStateRepository>,
    context: Option<Arc<EngineContext>>,
    transition_lock: Mutex<()>,
    persistence_failed: AtomicBool,
}

struct SystemGrant {
    fingerprint: String,
    expires_at: Instant,
    context: Option<retract_domain::ActiveContext>,
}

impl CleanerService {
    pub fn new(
        gateway: Arc<dyn TelegramGateway>,
        store: SecureJobStore,
    ) -> Result<Arc<Self>, AppError> {
        let persisted = store.load()?;
        let legacy_unbound = store.loaded_legacy_unbound();
        let plans: HashMap<Uuid, DeletionPlan> = persisted
            .plans
            .into_iter()
            .map(|plan| (plan.id, plan))
            .collect();
        let jobs = persisted
            .jobs
            .into_iter()
            .map(|mut job| {
                if let Some(plan) = plans.get(&job.plan_id) {
                    job.backfill_target_chat_ids(plan);
                }
                if legacy_unbound && matches!(job.status, JobStatus::Queued | JobStatus::Running) {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.retry_after_seconds = None;
                    push_error_once(&mut job, "legacy_store_requires_new_review");
                } else if matches!(job.status, JobStatus::Queued | JobStatus::Running)
                    && matches!(
                        job.operation,
                        PlanOperation::ClearHistory
                            | PlanOperation::ClearHistoryAndLeave
                            | PlanOperation::RemoveChatForSelf
                            | PlanOperation::DeleteBySender
                            | PlanOperation::DeleteGroup
                    )
                {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.retry_after_seconds = None;
                    push_error_once(&mut job, "restart_requires_new_review");
                } else if matches!(job.status, JobStatus::Running) {
                    job.status = JobStatus::Queued;
                    push_error_once(&mut job, "resumed_after_restart");
                }
                (job.id, job)
            })
            .collect();
        Ok(Arc::new(Self {
            gateway,
            plans: RwLock::new(plans),
            jobs: RwLock::new(jobs),
            cancellation: Mutex::new(HashMap::new()),
            system_grants: Mutex::new(HashMap::new()),
            store: Arc::new(LegacyTelegramRepository(store)),
            context: None,
            transition_lock: Mutex::new(()),
            persistence_failed: AtomicBool::new(false),
        }))
    }

    pub fn new_scoped(
        gateway: Arc<dyn TelegramGateway>,
        context: Arc<EngineContext>,
        store: Arc<dyn TelegramStateRepository>,
    ) -> Result<Arc<Self>, AppError> {
        context.check(gateway.as_ref())?;
        if store.scope() != Some(&context.active().scope) {
            return Err(crate::providers::telegram::engine_context::stale_context());
        }
        let persisted = store.load()?;
        // Recovery decisions are durable before any state or worker is published.
        store.save(&persisted)?;
        Ok(Arc::new(Self {
            gateway: Arc::new(SessionGateway {
                inner: gateway,
                context: context.clone(),
            }),
            plans: RwLock::new(persisted.plans.into_iter().map(|p| (p.id, p)).collect()),
            jobs: RwLock::new(persisted.jobs.into_iter().map(|j| (j.id, j)).collect()),
            cancellation: Mutex::new(HashMap::new()),
            system_grants: Mutex::new(HashMap::new()),
            store,
            context: Some(context),
            transition_lock: Mutex::new(()),
            persistence_failed: AtomicBool::new(false),
        }))
    }

    fn check_context(&self) -> Result<(), AppError> {
        if self.persistence_failed.load(Ordering::Acquire) {
            return Err(AppError::StatePersistenceFailed);
        }
        if let Some(context) = &self.context {
            context.check(self.gateway.as_ref())?;
        }
        Ok(())
    }

    pub(crate) fn is_bound_to(&self, context: &Arc<EngineContext>) -> bool {
        self.context
            .as_ref()
            .is_some_and(|bound| Arc::ptr_eq(bound, context))
    }

    async fn publish_plan(&self, mut plan: DeletionPlan) -> Result<PlanView, AppError> {
        self.check_context()?;
        if let Some(context) = &self.context {
            context.bind_plan(&mut plan)?;
        }
        let view = PlanView::from(&plan);
        self.transition(|plans, _| {
            plans.insert(plan.id, plan);
            Ok(())
        })
        .await?;
        Ok(view)
    }

    /// R3: serialize snapshot, mutation, durable save and publication together.
    /// Failed candidates never become executable or advance recoverable state.
    async fn transition<T>(
        &self,
        change: impl FnOnce(
            &mut HashMap<Uuid, DeletionPlan>,
            &mut HashMap<Uuid, JobRecord>,
        ) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let _serial = self.transition_lock.lock().await;
        if self.persistence_failed.load(Ordering::Acquire) {
            return Err(AppError::StatePersistenceFailed);
        }
        let mut plans = self.plans.read().await.clone();
        let mut jobs = self.jobs.read().await.clone();
        let result = change(&mut plans, &mut jobs)?;
        let state = PersistedState {
            plans: plans.values().cloned().collect(),
            jobs: jobs.values().cloned().collect(),
        };
        if self.store.save(&state).is_err() {
            self.persistence_failed.store(true, Ordering::Release);
            if let Some(context) = &self.context {
                context.quarantine();
            }
            return Err(AppError::StatePersistenceFailed);
        }
        *self.plans.write().await = plans;
        *self.jobs.write().await = jobs;
        Ok(result)
    }

    pub async fn snapshot(&self) -> Result<AppSnapshot, AppError> {
        let info = self.gateway.info();
        Ok(AppSnapshot {
            runtime_mode: info.mode.into(),
            account_label: info.account_label,
            mode_reason: info.reason,
            chats: self.gateway.chats().await?,
            recent_jobs: self.jobs().await,
            safety_notice:
                "Retract never downgrades a failed ‘delete for everyone’ request to ‘delete for me’."
                    .into(),
            auth: self.gateway.auth(),
        })
    }

    /// Return enough state to render the correct shell without waiting for the
    /// complete Telegram catalog. Test gateways keep their in-memory chat list
    /// so automated checks remain instantaneous and deterministic.
    pub async fn bootstrap_snapshot(&self) -> Result<AppSnapshot, AppError> {
        let info = self.gateway.info();
        let chats = if info.mode == "live" {
            Vec::new()
        } else {
            self.gateway.chats().await?
        };
        Ok(AppSnapshot {
            runtime_mode: info.mode.into(),
            account_label: info.account_label,
            mode_reason: info.reason,
            chats,
            recent_jobs: self.jobs().await,
            safety_notice:
                "Retract never downgrades a failed ‘delete for everyone’ request to ‘delete for me’."
                    .into(),
            auth: self.gateway.auth(),
        })
    }

    pub fn auth_snapshot(&self) -> AuthSnapshot {
        self.gateway.auth()
    }

    pub fn catalog_progress(&self) -> CatalogProgress {
        self.gateway.catalog_progress()
    }

    pub async fn search(&self, mut request: SearchRequest) -> Result<SearchResponse, AppError> {
        request.validate()?;
        let requested_limit = request.limit;
        let messages = self.gateway.search(&request).await?;
        let returned = messages.len();
        Ok(SearchResponse {
            messages,
            returned,
            truncated: returned == requested_limit,
        })
    }

    /// Refresh only the chats affected by a completed operation. Missing chats
    /// are intentionally omitted so the caller can remove them from its local
    /// list without rebuilding the complete Telegram catalog.
    pub async fn refresh_chats(
        &self,
        mut chat_ids: Vec<i64>,
    ) -> Result<Vec<ChatSummary>, AppError> {
        if chat_ids.len() > 1_000 || chat_ids.contains(&0) {
            return Err(AppError::InvalidRequest(
                "refresh up to 1,000 valid chats at a time".into(),
            ));
        }
        chat_ids.sort_unstable();
        chat_ids.dedup();
        let mut chats =
            stream::iter(chat_ids)
                .map(|chat_id| async move {
                    self.lookup_chat_with_timeout(chat_id, "chat refresh").await
                })
                .buffer_unordered(DIRECT_CHAT_LOOKUP_CONCURRENCY)
                .try_collect::<Vec<_>>()
                .await?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
        chats.sort_by_key(|chat| chat.title.to_lowercase());
        Ok(chats)
    }

    pub async fn prepare_selection(
        &self,
        request: PrepareSelectionRequest,
    ) -> Result<PlanView, AppError> {
        if request.message_refs.is_empty() || request.message_refs.len() > 100_000 {
            return Err(AppError::InvalidRequest(
                "select between 1 and 100,000 messages".into(),
            ));
        }
        let ids: Vec<_> = request
            .message_refs
            .into_iter()
            .map(
                |MessageRef {
                     chat_id,
                     message_id,
                 }| (chat_id, message_id),
            )
            .collect();
        let snapshots = self.gateway.messages_by_ids(&ids).await?;
        if snapshots.len()
            != ids
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
        {
            return Err(AppError::InvalidRequest(
                "one or more selected messages no longer exist".into(),
            ));
        }
        let plan = DeletionPlan::selected_messages(snapshots)?;
        self.publish_plan(plan).await
    }

    pub async fn prepare_chat_action(
        &self,
        request: PrepareChatActionRequest,
    ) -> Result<PlanView, AppError> {
        if matches!(
            request.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::ClearHistoryAndLeave
                | PlanOperation::DeleteAllMessagesAndLeave
        ) {
            return Err(AppError::InvalidRequest(
                "message-list plans use their dedicated preparation endpoint".into(),
            ));
        }
        let chat = self
            .lookup_chat_with_timeout(request.chat_id, "chat authority check")
            .await?
            .ok_or(AppError::NotFound)?;
        let plan = if request.operation == PlanOperation::LeaveChat {
            // Resolve the broadest safe cleanup scope before freezing the plan:
            // whole history, all admin-deletable IDs, or this account's IDs.
            let messages = if chat.capabilities.can_clear_for_everyone {
                Vec::new()
            } else if chat.capabilities.can_delete_others {
                self.gateway.chat_messages(chat.id).await?
            } else {
                self.gateway.own_messages(chat.id).await?
            };
            DeletionPlan::leave_chat(&chat, messages)?
        } else {
            DeletionPlan::chat_wide(request.operation, &chat)?
        };
        self.publish_plan(plan).await
    }

    pub async fn prepare_own_messages(&self, chat_id: i64) -> Result<PlanView, AppError> {
        let chat = self
            .lookup_chat_with_timeout(chat_id, "chat membership check")
            .await?
            .ok_or(AppError::NotFound)?;
        let active_group = matches!(
            chat.kind,
            cleaner_domain::ChatKind::BasicGroup | cleaner_domain::ChatKind::Supergroup
        ) && (chat.capabilities.role != cleaner_domain::ChatRole::Member
            || chat.capabilities.can_leave_chat);
        if !active_group {
            return Err(AppError::InvalidRequest(
                "deleting your complete message history is available only in groups you currently belong to"
                    .into(),
            ));
        }
        let messages = self.gateway.own_messages(chat_id).await?;
        if messages.is_empty() {
            return Err(AppError::InvalidRequest(
                "Telegram found no messages sent by your account in this group".into(),
            ));
        }
        let plan = DeletionPlan::own_messages(&chat, messages)?;
        self.publish_plan(plan).await
    }

    pub async fn prepare_sender_action(
        &self,
        request: PrepareSenderActionRequest,
    ) -> Result<PlanView, AppError> {
        let chat = self
            .lookup_chat_with_timeout(request.chat_id, "sender authority check")
            .await?
            .ok_or(AppError::NotFound)?;
        let sender_name = self.gateway.sender_name(request.sender_id).await?;
        let plan = DeletionPlan::by_sender(&chat, request.sender_id, sender_name)?;
        self.publish_plan(plan).await
    }

    pub async fn start_execution(
        self: &Arc<Self>,
        request: ExecuteRequest,
    ) -> Result<JobRecord, AppError> {
        self.check_context()?;
        let plan = self
            .plans
            .read()
            .await
            .get(&request.plan_id)
            .cloned()
            .ok_or(AppError::NotFound)?;
        plan.verify_confirmation(&ConfirmationProof {
            fingerprint: request.fingerprint,
            irreversible_acknowledged: request.irreversible_acknowledged,
            typed_chat_title: request.typed_chat_title,
        })?;

        let mut grants = self.system_grants.lock().await;
        self.check_context()?;
        let grant = grants.remove(&plan.id).ok_or_else(|| {
            AppError::SystemAuthentication(
                "confirm this frozen plan with macOS immediately before execution".into(),
            )
        })?;
        if grant.fingerprint != plan.fingerprint
            || grant.expires_at < Instant::now()
            || grant.context != self.context.as_ref().map(|c| c.active().clone())
        {
            return Err(AppError::SystemAuthentication(
                "the plan-bound authentication grant is invalid or expired".into(),
            ));
        }
        drop(grants);

        let job = JobRecord::new(&plan);
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut running = self.cancellation.lock().await;
        self.transition(|_, jobs| {
            self.check_context()?;
            if jobs.values().any(|existing| existing.plan_id == plan.id) {
                return Err(AppError::InvalidRequest(
                    "this frozen plan has already been started".into(),
                ));
            }
            jobs.insert(job.id, job.clone());
            Ok(())
        })
        .await?;
        running.insert(job.id, cancellation);
        drop(running);

        let service = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            service.run_job(job.id).await;
        });
        Ok(job)
    }

    pub async fn authorize_plan(&self, request: AuthorizePlanRequest) -> Result<(), AppError> {
        self.check_context()?;
        let plan = self
            .plans
            .read()
            .await
            .get(&request.plan_id)
            .cloned()
            .ok_or(AppError::NotFound)?;
        if request.fingerprint != plan.fingerprint {
            return Err(AppError::SystemAuthentication(
                "the authorization request does not match the frozen plan".into(),
            ));
        }
        let reason = authorization_reason(&plan);
        if let Some(context) = &self.context {
            context
                .authenticate(&reason, self.gateway.info().mode == "live")
                .await?;
        } else if self.gateway.info().mode == "live" {
            crate::local_auth::authenticate(&reason).await?;
        }
        let mut grants = self.system_grants.lock().await;
        self.check_context()?;
        grants.insert(
            plan.id,
            SystemGrant {
                fingerprint: plan.fingerprint,
                expires_at: Instant::now() + Duration::from_secs(60),
                context: self.context.as_ref().map(|c| c.active().clone()),
            },
        );
        Ok(())
    }

    pub async fn resume_incomplete(self: &Arc<Self>) {
        // Constructor-time restart policy may have stopped non-idempotent
        // broad jobs. Seal that decision before any safe frozen-ID job resumes.
        if self.check_context().is_err() || self.persist().await.is_err() {
            return;
        }
        let ids: Vec<_> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|job| matches!(job.status, JobStatus::Queued | JobStatus::Running))
            .map(|job| job.id)
            .collect();
        for id in ids {
            let mut running = self.cancellation.lock().await;
            if running.contains_key(&id) {
                continue;
            }
            running.insert(id, Arc::new(AtomicBool::new(false)));
            drop(running);
            let service = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                service.run_job(id).await;
            });
        }
    }

    async fn run_job(&self, job_id: Uuid) {
        if let Err(error) = self.run_job_inner(job_id).await {
            // A persistence failure has already quarantined this executor; the
            // committed snapshot remains authoritative and no second save is attempted.
            if !self.persistence_failed.load(Ordering::Acquire)
                && self
                    .transition(|_, jobs| {
                        if let Some(job) = jobs.get_mut(&job_id) {
                            job.status = if job.deleted > 0 {
                                JobStatus::Partial
                            } else {
                                JobStatus::Failed
                            };
                            job.retry_after_seconds = None;
                            job.updated_at = Utc::now();
                            push_error_once(job, error_code(&error));
                        }
                        Ok(())
                    })
                    .await
                    .is_err()
            {
                self.persistence_failed.store(true, Ordering::Release);
            }
        }
        self.cancellation.lock().await.remove(&job_id);
    }

    async fn run_job_inner(&self, job_id: Uuid) -> Result<(), AppError> {
        self.check_context()?;
        let cancellation = self
            .cancellation
            .lock()
            .await
            .get(&job_id)
            .cloned()
            .ok_or(AppError::StateUnavailable)?;
        let plan_id = self
            .transition(|_, jobs| {
                let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                if job.status.is_terminal() {
                    return Err(AppError::JobAlreadyTerminal);
                }
                job.status = JobStatus::Running;
                job.updated_at = Utc::now();
                Ok(job.plan_id)
            })
            .await?;

        let plan = self
            .plans
            .read()
            .await
            .get(&plan_id)
            .cloned()
            .ok_or(AppError::NotFound)?;

        if plan.operation == PlanOperation::ClearHistoryAndLeave
            && self
                .run_clear_history_and_leave(job_id, &plan, &cancellation)
                .await?
        {
            return Ok(());
        }

        let deletes_frozen_messages = matches!(
            plan.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::DeleteAllMessagesAndLeave
                | PlanOperation::LeaveChat
        );
        if deletes_frozen_messages
            && self
                .run_message_batches(job_id, &plan, &cancellation)
                .await?
        {
            return Ok(());
        }

        if !matches!(
            plan.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::ClearHistoryAndLeave
        ) {
            let operation = plan.operation;
            loop {
                if cancellation.load(Ordering::Acquire) {
                    self.finish_cancelled(job_id).await?;
                    return Ok(());
                }
                let chat_id = if matches!(
                    operation,
                    PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat
                ) {
                    plan.target_chat_id.ok_or_else(|| {
                        AppError::InvalidRequest(
                            "leave plan is missing its immutable chat ID".into(),
                        )
                    })?
                } else {
                    self.resolve_plan_chat(&plan).await?
                };
                if self.stop_if_cancelled(job_id, &cancellation).await? {
                    return Ok(());
                }
                let result = match operation {
                    PlanOperation::ClearHistory => {
                        self.gateway.clear_history_for_everyone(chat_id).await
                    }
                    PlanOperation::RemoveChatForSelf => {
                        self.gateway.remove_chat_for_self(chat_id).await
                    }
                    PlanOperation::DeleteGroup => self.gateway.delete_group(chat_id).await,
                    PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat => {
                        match self
                            .leave_and_remove_chat(job_id, chat_id, &cancellation)
                            .await
                        {
                            Ok(true) => return Ok(()),
                            Ok(false) => Ok(()),
                            Err(error) => Err(error),
                        }
                    }
                    PlanOperation::DeleteBySender => {
                        let sender_id = plan.target_sender_id.ok_or_else(|| {
                            AppError::InvalidRequest(
                                "sender-scoped plan is missing its sender ID".into(),
                            )
                        })?;
                        self.gateway
                            .delete_messages_by_sender(chat_id, sender_id)
                            .await
                    }
                    PlanOperation::SelectedMessages
                    | PlanOperation::DeleteMyMessages
                    | PlanOperation::ClearHistoryAndLeave => {
                        unreachable!()
                    }
                };
                match result {
                    Ok(()) => break,
                    Err(error) => {
                        if let Some(seconds) = telegram_retry_after(&error) {
                            if self.wait_for_retry(job_id, &cancellation, seconds).await? {
                                return Ok(());
                            }
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }

        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = if job.failed > 0 {
                JobStatus::Partial
            } else {
                JobStatus::Completed
            };
            job.retry_after_seconds = None;
            job.updated_at = Utc::now();
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Execute only the everyone-deletable portion of a frozen plan. Returning
    /// `true` means cancellation was recorded and any following chat action
    /// (notably leaving) must not run.
    async fn run_message_batches(
        &self,
        job_id: Uuid,
        plan: &DeletionPlan,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let batches = plan.everyone_batches(100)?;
        let next_batch = self
            .jobs
            .read()
            .await
            .get(&job_id)
            .map(|job| job.next_batch)
            .unwrap_or_default();
        for (index, batch) in batches.into_iter().enumerate().skip(next_batch) {
            loop {
                if cancellation.load(Ordering::Acquire) {
                    self.finish_cancelled(job_id).await?;
                    return Ok(true);
                }

                // Telegram capabilities can change after review. Recheck every ID on
                // every attempt, including after a FLOOD_WAIT pause.
                let mut allowed = Vec::new();
                let mut skipped = 0;
                let mut reach_error = None;
                for &message_id in &batch.message_ids {
                    match self.gateway.current_reach(batch.chat_id, message_id).await {
                        Ok(Some(DeletionReach::Everyone)) => allowed.push(message_id),
                        Ok(_) => skipped += 1,
                        Err(error) => {
                            reach_error = Some(error);
                            break;
                        }
                    }
                }

                if self.stop_if_cancelled(job_id, cancellation).await? {
                    return Ok(true);
                }

                let failed_count = if reach_error.is_some() {
                    batch.message_ids.len().saturating_sub(skipped)
                } else {
                    allowed.len()
                };
                let result = if let Some(error) = reach_error {
                    Err(error)
                } else if allowed.is_empty() {
                    Ok(())
                } else {
                    self.gateway
                        .delete_messages_for_everyone(batch.chat_id, &allowed)
                        .await
                };

                if let Err(error) = &result
                    && let Some(seconds) = telegram_retry_after(error)
                {
                    if self.wait_for_retry(job_id, cancellation, seconds).await? {
                        return Ok(true);
                    }
                    continue;
                }

                let fatal = result.as_ref().err().is_some_and(|error| {
                    matches!(error_code(error), "ambiguous_outcome" | "stale_context")
                });
                self.transition(|_, jobs| {
                    let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                    job.skipped += skipped;
                    job.next_batch = index + 1;
                    job.retry_after_seconds = None;
                    match result {
                        Ok(()) => job.deleted += allowed.len(),
                        Err(error) => {
                            if error_code(&error) == "ambiguous_outcome" {
                                job.uncertain += failed_count;
                            } else {
                                job.failed += failed_count;
                            }
                            push_error_once(job, error_code(&error));
                        }
                    }
                    if fatal {
                        job.status = if job.deleted > 0 {
                            JobStatus::Partial
                        } else {
                            JobStatus::Failed
                        };
                    }
                    job.updated_at = Utc::now();
                    Ok(())
                })
                .await?;
                if fatal {
                    return Ok(true);
                }
                break;
            }
        }
        Ok(false)
    }

    async fn run_clear_history_and_leave(
        &self,
        job_id: Uuid,
        plan: &DeletionPlan,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let next_phase = self
            .jobs
            .read()
            .await
            .get(&job_id)
            .map(|job| job.next_batch)
            .unwrap_or_default();
        if next_phase == 0 {
            loop {
                if cancellation.load(Ordering::Acquire) {
                    self.finish_cancelled(job_id).await?;
                    return Ok(true);
                }
                let chat_id = self.resolve_plan_chat(plan).await?;
                if self.stop_if_cancelled(job_id, cancellation).await? {
                    return Ok(true);
                }
                match self
                    .gateway
                    .clear_history_for_everyone_keep_chat(chat_id)
                    .await
                {
                    Ok(()) => {
                        self.transition(|_, jobs| {
                            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                            job.next_batch = 1;
                            job.updated_at = Utc::now();
                            Ok(())
                        })
                        .await?;
                        break;
                    }
                    Err(error) => {
                        if let Some(seconds) = telegram_retry_after(&error) {
                            if self.wait_for_retry(job_id, cancellation, seconds).await? {
                                return Ok(true);
                            }
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }

        loop {
            if cancellation.load(Ordering::Acquire) {
                self.finish_cancelled(job_id).await?;
                return Ok(true);
            }
            let chat_id = plan.target_chat_id.ok_or_else(|| {
                AppError::InvalidRequest("leave plan is missing its immutable chat ID".into())
            })?;
            match self
                .leave_and_remove_chat(job_id, chat_id, cancellation)
                .await
            {
                Ok(cancelled) => return Ok(cancelled),
                Err(error) => {
                    if let Some(seconds) = telegram_retry_after(&error) {
                        if self.wait_for_retry(job_id, cancellation, seconds).await? {
                            return Ok(true);
                        }
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    async fn leave_and_remove_chat(
        &self,
        job_id: Uuid,
        chat_id: i64,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let current = self
            .lookup_chat_with_timeout(chat_id, "leave-and-remove state check")
            .await?;
        if self.stop_if_cancelled(job_id, cancellation).await? {
            return Ok(true);
        }

        let Some(current) = current else {
            return Ok(false);
        };

        let refreshed = if current.capabilities.can_leave_chat {
            self.gateway.leave_chat(chat_id).await?;
            // Membership removal is an acknowledged compound step. A failed
            // required save here must prevent the following self-removal call.
            self.transition(|_, jobs| {
                jobs.get_mut(&job_id).ok_or(AppError::NotFound)?.updated_at = Utc::now();
                Ok(())
            })
            .await?;
            if self.stop_if_cancelled(job_id, cancellation).await? {
                return Ok(true);
            }

            self.lookup_chat_with_timeout(chat_id, "post-leave cleanup check")
                .await?
        } else if current.capabilities.can_remove_for_self {
            Some(current)
        } else {
            return Err(AppError::Gateway("CHAT_MEMBER_REQUIRED".into()));
        };

        if self.stop_if_cancelled(job_id, cancellation).await? {
            return Ok(true);
        }

        match refreshed {
            None => Ok(false),
            Some(chat) if chat.capabilities.can_remove_for_self => {
                self.gateway.remove_chat_for_self(chat_id).await?;
                Ok(false)
            }
            Some(_) => Err(AppError::Gateway("CHAT_DELETE_FOR_SELF_FORBIDDEN".into())),
        }
    }

    async fn resolve_plan_chat(&self, plan: &DeletionPlan) -> Result<i64, AppError> {
        let target_chat_id = plan.target_chat_id.ok_or_else(|| {
            AppError::InvalidRequest("chat-wide plan is missing its immutable chat ID".into())
        })?;
        let current = self
            .lookup_chat_with_timeout(target_chat_id, "execution-time authority check")
            .await?
            .ok_or(AppError::NotFound)?;
        let still_allowed = match plan.operation {
            PlanOperation::ClearHistory => current.capabilities.can_clear_for_everyone,
            PlanOperation::ClearHistoryAndLeave => {
                current.capabilities.can_clear_for_everyone && current.capabilities.can_leave_chat
            }
            PlanOperation::DeleteAllMessagesAndLeave => {
                current.capabilities.can_delete_others && current.capabilities.can_leave_chat
            }
            PlanOperation::RemoveChatForSelf => current.capabilities.can_remove_for_self,
            PlanOperation::DeleteGroup => current.capabilities.can_delete_group,
            PlanOperation::LeaveChat => current.capabilities.can_leave_chat,
            PlanOperation::DeleteBySender => current.capabilities.can_delete_by_sender,
            PlanOperation::DeleteMyMessages => false,
            PlanOperation::SelectedMessages => false,
        };
        if !still_allowed {
            let error = match plan.operation {
                PlanOperation::ClearHistoryAndLeave
                | PlanOperation::DeleteAllMessagesAndLeave
                | PlanOperation::LeaveChat => "CHAT_MEMBER_REQUIRED",
                PlanOperation::RemoveChatForSelf => "CHAT_DELETE_FOR_SELF_FORBIDDEN",
                _ => "CHAT_ADMIN_REQUIRED",
            };
            return Err(AppError::Gateway(error.into()));
        }
        Ok(target_chat_id)
    }

    async fn lookup_chat_with_timeout(
        &self,
        chat_id: i64,
        operation: &str,
    ) -> Result<Option<ChatSummary>, AppError> {
        tokio::time::timeout(DIRECT_CHAT_LOOKUP_TIMEOUT, self.gateway.chat_by_id(chat_id))
            .await
            .map_err(|_| {
                AppError::Timeout(format!(
                    "Telegram did not answer the {operation} within {} seconds. Try again.",
                    DIRECT_CHAT_LOOKUP_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn finish_cancelled(&self, job_id: Uuid) -> Result<(), AppError> {
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Cancelled;
            job.retry_after_seconds = None;
            job.updated_at = Utc::now();
            Ok(())
        })
        .await
    }

    async fn stop_if_cancelled(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        if !cancellation.load(Ordering::Acquire) {
            return Ok(false);
        }
        self.finish_cancelled(job_id).await?;
        Ok(true)
    }

    async fn wait_for_retry(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
        seconds: u64,
    ) -> Result<bool, AppError> {
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Queued;
            job.retry_after_seconds = Some(seconds);
            job.updated_at = Utc::now();
            push_error_once(job, "telegram_rate_limited");
            Ok(())
        })
        .await?;

        for _ in 0..seconds {
            if cancellation.load(Ordering::Acquire) {
                self.finish_cancelled(job_id).await?;
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            self.check_context()?;
        }
        if cancellation.load(Ordering::Acquire) {
            self.finish_cancelled(job_id).await?;
            return Ok(true);
        }

        self.check_context()?;
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Running;
            job.retry_after_seconds = None;
            job.updated_at = Utc::now();
            Ok(())
        })
        .await?;
        Ok(false)
    }

    pub async fn cancel_job(&self, job_id: Uuid) -> Result<JobRecord, AppError> {
        let jobs = self.jobs.read().await;
        let job = jobs.get(&job_id).cloned().ok_or(AppError::NotFound)?;
        if job.status.is_terminal() {
            return Err(AppError::JobAlreadyTerminal);
        }
        drop(jobs);
        let cancellation = self
            .cancellation
            .lock()
            .await
            .get(&job_id)
            .cloned()
            .ok_or(AppError::StateUnavailable)?;
        cancellation.store(true, Ordering::Release);
        Ok(job)
    }

    pub async fn jobs(&self) -> Vec<JobRecord> {
        let mut jobs: Vec<_> = self.jobs.read().await.values().cloned().collect();
        if self.persistence_failed.load(Ordering::Acquire) {
            for job in &mut jobs {
                if !job.status.is_terminal() {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.retry_after_seconds = None;
                    push_error_once(job, "state_persistence_failed");
                }
            }
        }
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs.truncate(50);
        jobs
    }

    pub async fn request_qr_auth(&self) -> Result<(), AppError> {
        self.gateway.request_qr_auth().await
    }

    pub async fn submit_phone(&self, value: &str) -> Result<(), AppError> {
        self.gateway.submit_phone(value).await
    }

    pub async fn submit_email_address(&self, value: &str) -> Result<(), AppError> {
        self.gateway.submit_email_address(value).await
    }

    pub async fn submit_email_code(&self, value: &str) -> Result<(), AppError> {
        self.gateway.submit_email_code(value).await
    }

    pub async fn submit_code(&self, value: &str) -> Result<(), AppError> {
        self.gateway.submit_code(value).await
    }

    pub async fn submit_password(&self, value: &str) -> Result<(), AppError> {
        self.gateway.submit_password(value).await
    }

    pub async fn shutdown(&self) {
        let _ = self.gateway.close().await;
        let _ = self.persist().await;
    }

    async fn persist(&self) -> Result<(), AppError> {
        self.transition(|_, _| Ok(())).await
    }
}

fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Gateway(message) if message == "RETRACT_AMBIGUOUS_OUTCOME" => "ambiguous_outcome",
        AppError::InvalidRequest(message) if message == "stale_context" => "stale_context",
        AppError::Gateway(_) => "telegram_rejected",
        AppError::Timeout(_) => "telegram_timeout",
        AppError::SecureStore(_) => "secure_store",
        AppError::SystemAuthentication(_) => "system_authentication",
        AppError::NotFound => "not_found",
        AppError::JobAlreadyTerminal => "job_terminal",
        AppError::Domain(_) | AppError::InvalidRequest(_) => "invalid_plan",
        AppError::StateUnavailable => "state_unavailable",
        AppError::ProfileInUse => "profile_in_use",
        AppError::StatePersistenceFailed => "state_persistence_failed",
    }
}

fn authorization_reason(plan: &DeletionPlan) -> String {
    let plan_token = &plan.fingerprint[..plan.fingerprint.len().min(12)];
    let chat_id = plan.target_chat_id.unwrap_or_default();
    let chat = trusted_prompt_label(plan.chat_title.as_deref().unwrap_or("Unknown chat"));
    let target = format!("‘{chat}’ (chat {chat_id})");
    let action = match plan.operation {
        PlanOperation::DeleteGroup => format!("Permanently delete {target}"),
        PlanOperation::ClearHistoryAndLeave => {
            format!("Clear all history for everyone in {target}, then leave")
        }
        PlanOperation::DeleteAllMessagesAndLeave => format!(
            "Delete {} frozen messages in {target}, then leave",
            plan.summary.delete_for_everyone
        ),
        PlanOperation::LeaveChat => format!(
            "Delete {} frozen messages in {target}, then leave",
            plan.summary.delete_for_everyone
        ),
        PlanOperation::ClearHistory => {
            format!("Clear all Telegram history for everyone in {target}")
        }
        PlanOperation::RemoveChatForSelf => {
            format!("Remove {target} only from this account")
        }
        PlanOperation::DeleteBySender => {
            let sender = trusted_prompt_label(
                plan.target_sender_name
                    .as_deref()
                    .unwrap_or("Unknown sender"),
            );
            let sender_id = plan.target_sender_id.unwrap_or_default();
            format!("Delete every message by ‘{sender}’ (sender {sender_id}) in {target}")
        }
        PlanOperation::DeleteMyMessages => format!(
            "Delete {} frozen messages sent by your account in {target}",
            plan.summary.delete_for_everyone
        ),
        PlanOperation::SelectedMessages => {
            let mut chat_ids = plan
                .items
                .iter()
                .filter(|item| item.expected_reach == DeletionReach::Everyone)
                .map(|item| item.chat_id)
                .collect::<Vec<_>>();
            chat_ids.sort_unstable();
            chat_ids.dedup();
            let visible_ids = chat_ids
                .iter()
                .take(4)
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let remainder = chat_ids.len().saturating_sub(4);
            let suffix = if remainder == 0 {
                String::new()
            } else {
                format!(" and {remainder} more")
            };
            format!(
                "Delete {} frozen Telegram messages for everyone in chat IDs {visible_ids}{suffix}",
                plan.summary.delete_for_everyone
            )
        }
    };
    format!("Plan {plan_token}: {action}.")
}

fn trusted_prompt_label(value: &str) -> String {
    let single_line = value
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
        })
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    single_line.trim().chars().take(80).collect()
}

fn telegram_retry_after(error: &AppError) -> Option<u64> {
    let AppError::Gateway(message) = error else {
        return None;
    };
    let normalized = message.to_ascii_lowercase();
    if !normalized.contains("flood_wait")
        && !normalized.contains("retry after")
        && !normalized.contains("too many requests")
        && !normalized.contains("429")
    {
        return None;
    }
    let seconds = message
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u64>().ok())
        .rfind(|number| *number != 429)
        .unwrap_or(5);
    Some(seconds.clamp(1, 86_400))
}

fn push_error_once(job: &mut JobRecord, code: &str) {
    if !job.error_codes.iter().any(|existing| existing == code) {
        job.error_codes.push(code.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        demo_gateway::{DemoGateway, TestFailurePoint},
        secure_store::SecureJobStore,
    };
    use cleaner_domain::{ContentKind, MessageSnapshot, PlanOperation};

    const TERMINAL_JOB_TIMEOUT: Duration = Duration::from_secs(10);

    async fn prepared_broad_restart_plan(
        service: &Arc<CleanerService>,
        operation: PlanOperation,
    ) -> DeletionPlan {
        let view = match operation {
            PlanOperation::ClearHistory => service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1001,
                    operation,
                })
                .await
                .unwrap(),
            PlanOperation::ClearHistoryAndLeave => service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1002,
                    operation: PlanOperation::LeaveChat,
                })
                .await
                .unwrap(),
            PlanOperation::RemoveChatForSelf => service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: 304,
                    operation,
                })
                .await
                .unwrap(),
            PlanOperation::DeleteBySender => service
                .prepare_sender_action(PrepareSenderActionRequest {
                    chat_id: -1001,
                    sender_id: 714,
                })
                .await
                .unwrap(),
            PlanOperation::DeleteGroup => service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1001,
                    operation,
                })
                .await
                .unwrap(),
            _ => panic!("unsupported broad restart operation: {operation:?}"),
        };
        let plan = service.plans.read().await.get(&view.id).cloned().unwrap();
        assert_eq!(plan.operation, operation);
        plan
    }

    async fn wait_for_terminal_job(service: &CleanerService, job_id: Uuid) -> JobRecord {
        tokio::time::timeout(TERMINAL_JOB_TIMEOUT, async {
            loop {
                let job = service
                    .jobs()
                    .await
                    .into_iter()
                    .find(|candidate| candidate.id == job_id)
                    .unwrap();
                if job.status.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "job {job_id} did not become terminal within {} seconds",
                TERMINAL_JOB_TIMEOUT.as_secs()
            )
        })
    }

    async fn wait_for_persisted_terminal_job(
        path: &std::path::Path,
        key: [u8; 32],
        job_id: Uuid,
    ) -> PersistedState {
        tokio::time::timeout(TERMINAL_JOB_TIMEOUT, async {
            loop {
                let state = SecureJobStore::with_test_key(path.to_path_buf(), key)
                    .load()
                    .unwrap();
                if state
                    .jobs
                    .iter()
                    .any(|job| job.id == job_id && job.status.is_terminal())
                {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "job {job_id} was not durably terminal within {} seconds",
                TERMINAL_JOB_TIMEOUT.as_secs()
            )
        })
    }

    async fn wait_for_rate_limited_job(service: &CleanerService, job_id: Uuid) -> JobRecord {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let job = service
                    .jobs()
                    .await
                    .into_iter()
                    .find(|candidate| candidate.id == job_id)
                    .unwrap();
                if job.status == JobStatus::Queued && job.retry_after_seconds.is_some() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("job {job_id} did not enter its bounded retry wait"))
    }

    async fn wait_for_current_reach_check(gateway: &DemoGateway) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if gateway.current_reach_started() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the synthetic reach preflight did not start");
    }

    #[test]
    fn extracts_bounded_telegram_retry_delays() {
        assert_eq!(
            telegram_retry_after(&AppError::Gateway(
                "429 Too Many Requests: retry after 17".into()
            )),
            Some(17)
        );
        assert_eq!(
            telegram_retry_after(&AppError::Gateway("FLOOD_WAIT_999999".into())),
            Some(86_400)
        );
        assert_eq!(
            telegram_retry_after(&AppError::Gateway("CHAT_ADMIN_REQUIRED".into())),
            None
        );
    }

    #[test]
    fn persisted_cleanup_state_contains_recovery_metadata_but_no_message_content() {
        const PRIVATE_PREVIEW: &str = "SYNTHETIC_CONTENT_MUST_NOT_PERSIST";

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [34; 32];
        let plan = DeletionPlan::selected_messages(vec![MessageSnapshot {
            chat_id: -1001,
            message_id: 90_001,
            sender_id: 714,
            sender_name: "Synthetic Sender".into(),
            sent_at: Utc::now(),
            is_outgoing: false,
            content_kind: ContentKind::File,
            preview: PRIVATE_PREVIEW.into(),
            privacy_findings: Vec::new(),
            album_id: Some(81_001),
            is_pinned: false,
            deletion_reach: DeletionReach::Everyone,
        }])
        .unwrap();
        let mut job = JobRecord::new(&plan);
        job.status = JobStatus::Partial;
        job.deleted = 1;
        job.next_batch = 1;
        job.retry_after_seconds = Some(17);
        job.error_codes = vec!["synthetic_retry_exhausted".into()];
        job.updated_at = job.created_at + chrono::Duration::seconds(5);

        SecureJobStore::with_test_key(path.clone(), key)
            .save(&PersistedState {
                plans: vec![plan.clone()],
                jobs: vec![job.clone()],
            })
            .unwrap();
        let reloaded = SecureJobStore::with_test_key(path, key).load().unwrap();

        assert_eq!(reloaded.plans.len(), 1);
        assert_eq!(reloaded.jobs.len(), 1);
        let reloaded_plan = &reloaded.plans[0];
        let reloaded_job = &reloaded.jobs[0];
        assert_eq!(reloaded_plan.id, plan.id);
        assert_eq!(reloaded_plan.fingerprint, plan.fingerprint);
        assert_eq!(reloaded_plan.operation, PlanOperation::SelectedMessages);
        assert_eq!(reloaded_plan.items.len(), 1);
        assert_eq!(reloaded_plan.items[0].chat_id, -1001);
        assert_eq!(reloaded_plan.items[0].message_id, 90_001);
        assert_eq!(
            reloaded_plan.items[0].expected_reach,
            DeletionReach::Everyone
        );
        assert_eq!(reloaded_plan.summary.selected, 1);
        assert_eq!(reloaded_plan.summary.delete_for_everyone, 1);
        assert_eq!(reloaded_plan.created_at, plan.created_at);
        assert_eq!(reloaded_job.id, job.id);
        assert_eq!(reloaded_job.plan_id, plan.id);
        assert_eq!(reloaded_job.operation, PlanOperation::SelectedMessages);
        assert_eq!(reloaded_job.target_chat_ids, vec![-1001]);
        assert_eq!(reloaded_job.status, JobStatus::Partial);
        assert_eq!(
            (
                reloaded_job.total,
                reloaded_job.deleted,
                reloaded_job.skipped,
                reloaded_job.failed,
                reloaded_job.next_batch,
            ),
            (1, 1, 0, 0, 1)
        );
        assert_eq!(reloaded_job.retry_after_seconds, Some(17));
        assert_eq!(reloaded_job.error_codes, vec!["synthetic_retry_exhausted"]);
        assert_eq!(reloaded_job.created_at, job.created_at);
        assert_eq!(reloaded_job.updated_at, job.updated_at);

        let serialized = serde_json::to_string(&reloaded).unwrap();
        assert!(!serialized.contains(PRIVATE_PREVIEW));
        for forbidden in [
            "preview",
            "text",
            "caption",
            "fileName",
            "attachment",
            "apiHash",
            "password",
            "authCode",
        ] {
            assert!(!serialized.contains(&format!("\"{forbidden}\"")));
        }
    }

    #[test]
    fn high_impact_plan_requires_a_bound_single_use_grant() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [19; 32]);
            let gateway: Arc<dyn TelegramGateway> = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway, store).unwrap();
            let chat = service
                .snapshot()
                .await
                .unwrap()
                .chats
                .into_iter()
                .find(|chat| chat.capabilities.can_clear_for_everyone)
                .unwrap();
            let plan = service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: chat.id,
                    operation: PlanOperation::ClearHistory,
                })
                .await
                .unwrap();
            let execution = || ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
                irreversible_acknowledged: true,
                typed_chat_title: plan.chat_title.clone(),
            };

            assert!(matches!(
                service.start_execution(execution()).await,
                Err(AppError::SystemAuthentication(_))
            ));
            assert!(
                service
                    .authorize_plan(AuthorizePlanRequest {
                        plan_id: plan.id,
                        fingerprint: "altered".into(),
                    })
                    .await
                    .is_err()
            );
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            let job = service.start_execution(execution()).await.unwrap();

            wait_for_terminal_job(&service, job.id).await;
            assert!(matches!(
                service.start_execution(execution()).await,
                Err(AppError::SystemAuthentication(_))
            ));
            assert_eq!(service.jobs().await.len(), 1);
        });
    }

    #[test]
    fn selected_message_plan_requires_a_bound_single_use_grant() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [21; 32]);
            let gateway: Arc<dyn TelegramGateway> = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway, store).unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: 101,
                        message_id: 1,
                    }],
                })
                .await
                .unwrap();
            assert_eq!(
                plan.confirmation_tier,
                cleaner_domain::ConfirmationTier::Low
            );
            let execution = || ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
                irreversible_acknowledged: true,
                typed_chat_title: None,
            };

            assert!(matches!(
                service.start_execution(execution()).await,
                Err(AppError::SystemAuthentication(_))
            ));
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            assert!(service.start_execution(execution()).await.is_ok());
        });
    }

    #[test]
    fn native_authorization_reason_identifies_the_exact_frozen_target() {
        tauri::async_runtime::block_on(async {
            let gateway = DemoGateway::new();
            let chat = gateway.chat_by_id(-1001).await.unwrap().unwrap();
            let plan = DeletionPlan::by_sender(&chat, 714, "Priya".into()).unwrap();
            let reason = authorization_reason(&plan);

            assert!(reason.contains("Design Team"));
            assert!(reason.contains("-1001"));
            assert!(reason.contains("Priya"));
            assert!(reason.contains("714"));
            assert!(reason.contains(&plan.fingerprint[..12]));
        });
    }

    #[test]
    fn native_authorization_labels_strip_line_and_direction_controls() {
        assert_eq!(
            trusted_prompt_label("Design\nTeam \u{202e}123\u{2069}"),
            "Design Team 123"
        );
    }

    #[test]
    fn sender_plan_uses_the_backend_resolved_display_name() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [22; 32]);
            let gateway: Arc<dyn TelegramGateway> = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway, store).unwrap();
            let plan = service
                .prepare_sender_action(PrepareSenderActionRequest {
                    chat_id: -1001,
                    sender_id: 714,
                })
                .await
                .unwrap();

            assert_eq!(plan.target_sender_name.as_deref(), Some("Priya"));
        });
    }

    #[test]
    fn cancellation_during_capability_refresh_prevents_the_destructive_call() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [24; 32]);
            let gateway = Arc::new(DemoGateway::new());
            gateway.delay_current_reach(100);
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: 101,
                        message_id: 1,
                    }],
                })
                .await
                .unwrap();
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None,
                })
                .await
                .unwrap();

            wait_for_current_reach_check(&gateway).await;
            service.cancel_job(job.id).await.unwrap();
            let finished = wait_for_terminal_job(&service, job.id).await;
            assert_eq!(finished.status, JobStatus::Cancelled);
            assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
        });
    }

    #[test]
    fn selected_everyone_deletion_never_downgrades_after_reach_changes() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [32; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: 101,
                        message_id: 1,
                    }],
                })
                .await
                .unwrap();

            gateway
                .set_message_reach(101, 1, DeletionReach::SelfOnly)
                .await;
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            gateway.clear_operation_log().await;
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None,
                })
                .await
                .unwrap();

            wait_for_terminal_job(&service, job.id).await;

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            let operations = gateway.operation_log().await;
            assert!(
                operations.is_empty(),
                "unexpected destructive operations: {operations:?}"
            );
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 0);
            assert_eq!(finished.skipped, 1);
            assert_eq!(finished.failed, 0);
            assert!(finished.error_codes.is_empty());
            assert!(finished.retry_after_seconds.is_none());
            assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
        });
    }

    #[test]
    fn selected_everyone_deletion_rechecks_reach_after_a_rate_limit_wait() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [41; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: 101,
                        message_id: 1,
                    }],
                })
                .await
                .unwrap();
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            gateway.clear_test_traces().await;
            gateway
                .inject_rate_limit_once(TestFailurePoint::DeleteMessagesForEveryone)
                .await;

            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None,
                })
                .await
                .unwrap();
            gateway.wait_for_injected_failure(1).await;
            let waiting = wait_for_rate_limited_job(&service, job.id).await;
            assert_eq!(waiting.retry_after_seconds, Some(1));
            assert_eq!(waiting.error_codes, ["telegram_rate_limited"]);

            gateway
                .set_message_reach(101, 1, DeletionReach::SelfOnly)
                .await;
            let finished = wait_for_terminal_job(&service, job.id).await;

            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 0);
            assert_eq!(finished.skipped, 1);
            assert_eq!(finished.failed, 0);
            assert_eq!(finished.next_batch, 1);
            assert!(finished.retry_after_seconds.is_none());
            assert_eq!(finished.error_codes, ["telegram_rate_limited"]);
            assert_eq!(
                gateway.current_reach_calls().await,
                vec![(101, 1), (101, 1)]
            );
            assert_eq!(
                gateway.operation_log().await,
                vec!["delete_messages_for_everyone:101:1"]
            );
            assert_eq!(gateway.delete_calls().await, vec![(101, vec![1])]);
            assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
        });
    }

    #[test]
    fn chat_wide_deletion_rechecks_authority_after_a_rate_limit_wait() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [42; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1001,
                    operation: PlanOperation::ClearHistory,
                })
                .await
                .unwrap();
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            gateway.clear_test_traces().await;
            gateway
                .inject_rate_limit_once(TestFailurePoint::ClearHistoryForEveryone)
                .await;

            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: plan.chat_title,
                })
                .await
                .unwrap();
            gateway.wait_for_injected_failure(1).await;
            let waiting = wait_for_rate_limited_job(&service, job.id).await;
            assert_eq!(waiting.status, JobStatus::Queued);
            assert_eq!(waiting.retry_after_seconds, Some(1));
            assert_eq!(waiting.error_codes, ["telegram_rate_limited"]);

            gateway.set_chat_clear_authority(-1001, false).await;
            let finished = wait_for_terminal_job(&service, job.id).await;

            assert_eq!(finished.status, JobStatus::Failed);
            assert_eq!(finished.total, 0);
            assert_eq!(finished.deleted, 0);
            assert_eq!(finished.skipped, 0);
            assert_eq!(finished.failed, 0);
            assert_eq!(finished.next_batch, 0);
            assert!(finished.retry_after_seconds.is_none());
            assert_eq!(
                finished.error_codes,
                ["telegram_rate_limited", "telegram_rejected"]
            );
            assert_eq!(gateway.chat_by_id_calls().await, vec![-1001, -1001]);
            assert_eq!(
                gateway.operation_log().await,
                vec!["clear_history_for_everyone:-1001"]
            );
            assert!(gateway.chat_by_id(-1001).await.unwrap().is_some());
        });
    }

    #[test]
    fn selected_message_job_uses_exact_telegram_batch_boundaries() {
        tauri::async_runtime::block_on(async {
            const CHAT_ID: i64 = 101;
            const FIRST_MESSAGE_ID: i64 = 50_000;
            const MESSAGE_COUNT: usize = 205;

            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [33; 32]);
            let gateway = Arc::new(DemoGateway::new());
            gateway
                .append_messages(CHAT_ID, FIRST_MESSAGE_ID, MESSAGE_COUNT)
                .await;
            let expected_refs = (0..MESSAGE_COUNT)
                .map(|offset| MessageRef {
                    chat_id: CHAT_ID,
                    message_id: FIRST_MESSAGE_ID + offset as i64,
                })
                .collect::<Vec<_>>();
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: expected_refs.clone(),
                })
                .await
                .unwrap();

            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            gateway.clear_operation_log().await;
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None,
                })
                .await
                .unwrap();

            wait_for_terminal_job(&service, job.id).await;

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, MESSAGE_COUNT);
            assert_eq!(finished.skipped, 0);
            assert_eq!(finished.failed, 0);
            assert!(finished.error_codes.is_empty());
            assert!(finished.retry_after_seconds.is_none());
            assert_eq!(gateway.delete_batch_sizes().await, vec![100, 100, 5]);
            assert_eq!(
                gateway.delete_calls().await,
                vec![
                    (CHAT_ID, (50_000..=50_099).collect::<Vec<_>>()),
                    (CHAT_ID, (50_100..=50_199).collect::<Vec<_>>()),
                    (CHAT_ID, (50_200..=50_204).collect::<Vec<_>>()),
                ]
            );

            let expected_operations = expected_refs
                .chunks(100)
                .map(|batch| {
                    let message_ids = batch
                        .iter()
                        .map(|message| message.message_id.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("delete_messages_for_everyone:{CHAT_ID}:{message_ids}")
                })
                .collect::<Vec<_>>();
            assert_eq!(gateway.operation_log().await, expected_operations);
        });
    }

    #[test]
    fn restart_requires_new_review_for_every_non_idempotent_broad_operation() {
        tauri::async_runtime::block_on(async {
            for operation in [
                PlanOperation::ClearHistory,
                PlanOperation::ClearHistoryAndLeave,
                PlanOperation::RemoveChatForSelf,
                PlanOperation::DeleteBySender,
                PlanOperation::DeleteGroup,
            ] {
                for (persisted_status, deleted, expected_status) in [
                    (JobStatus::Queued, 0, JobStatus::Failed),
                    (JobStatus::Running, 1, JobStatus::Partial),
                ] {
                    let directory = tempfile::tempdir().unwrap();
                    let path = directory.path().join("jobs.enc");
                    let key = [35; 32];
                    let gateway = Arc::new(DemoGateway::new());
                    let preparation = CleanerService::new(
                        gateway.clone(),
                        SecureJobStore::with_test_key(path.clone(), key),
                    )
                    .unwrap();
                    let plan = prepared_broad_restart_plan(&preparation, operation).await;
                    let mut job = JobRecord::new(&plan);
                    job.status = persisted_status;
                    job.total = 1;
                    job.deleted = deleted;
                    let job_id = job.id;
                    drop(preparation);
                    SecureJobStore::with_test_key(path.clone(), key)
                        .save(&PersistedState {
                            plans: vec![plan],
                            jobs: vec![job],
                        })
                        .unwrap();

                    gateway.clear_operation_log().await;
                    let service = CleanerService::new(
                        gateway.clone(),
                        SecureJobStore::with_test_key(path.clone(), key),
                    )
                    .unwrap();
                    service.resume_incomplete().await;

                    let interrupted = service
                        .jobs()
                        .await
                        .into_iter()
                        .find(|candidate| candidate.id == job_id)
                        .unwrap();
                    assert_eq!(
                        interrupted.status, expected_status,
                        "unexpected restart status for {operation:?} from {persisted_status:?}"
                    );
                    assert_eq!(interrupted.deleted, deleted);
                    assert_eq!(interrupted.retry_after_seconds, None);
                    assert_eq!(
                        interrupted.error_codes,
                        vec!["restart_requires_new_review"],
                        "unexpected restart diagnostic for {operation:?} from {persisted_status:?}"
                    );
                    assert!(
                        gateway.operation_log().await.is_empty(),
                        "restart replayed {operation:?} from {persisted_status:?}"
                    );

                    let reloaded = SecureJobStore::with_test_key(path, key).load().unwrap();
                    assert_eq!(reloaded.jobs.len(), 1);
                    assert_eq!(reloaded.jobs[0].status, expected_status);
                    assert_eq!(
                        reloaded.jobs[0].error_codes,
                        vec!["restart_requires_new_review"]
                    );
                }
            }
        });
    }

    #[test]
    fn restart_resumes_selected_message_job_from_frozen_ids() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("jobs.enc");
            let key = [36; 32];
            let gateway = Arc::new(DemoGateway::new());
            let preparation = CleanerService::new(
                gateway.clone(),
                SecureJobStore::with_test_key(path.clone(), key),
            )
            .unwrap();
            let view = preparation
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: 101,
                        message_id: 1,
                    }],
                })
                .await
                .unwrap();
            let plan = preparation
                .plans
                .read()
                .await
                .get(&view.id)
                .cloned()
                .unwrap();
            let mut job = JobRecord::new(&plan);
            job.status = JobStatus::Running;
            let job_id = job.id;
            drop(preparation);
            SecureJobStore::with_test_key(path.clone(), key)
                .save(&PersistedState {
                    plans: vec![plan],
                    jobs: vec![job],
                })
                .unwrap();

            gateway.clear_operation_log().await;
            let service =
                CleanerService::new(gateway.clone(), SecureJobStore::with_test_key(path, key))
                    .unwrap();
            service.resume_incomplete().await;

            let finished = wait_for_terminal_job(&service, job_id).await;
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 1);
            assert!(
                finished
                    .error_codes
                    .iter()
                    .any(|code| code == "resumed_after_restart")
            );
            assert_eq!(
                gateway.operation_log().await,
                vec!["delete_messages_for_everyone:101:1"]
            );
            assert!(
                gateway
                    .messages_by_ids(&[(101, 1)])
                    .await
                    .unwrap()
                    .is_empty()
            );
        });
    }

    #[test]
    fn restart_resumes_selected_message_job_from_nonzero_batch_cursor() {
        tauri::async_runtime::block_on(async {
            const CHAT_ID: i64 = 101;
            const FIRST_MESSAGE_ID: i64 = 70_000;
            const MESSAGE_COUNT: usize = 205;
            const COMPLETED_BATCH_SIZE: usize = 100;

            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("jobs.enc");
            let key = [38; 32];
            let gateway = Arc::new(DemoGateway::new());
            gateway
                .append_messages(CHAT_ID, FIRST_MESSAGE_ID, MESSAGE_COUNT)
                .await;
            let message_refs = (0..MESSAGE_COUNT)
                .map(|offset| MessageRef {
                    chat_id: CHAT_ID,
                    message_id: FIRST_MESSAGE_ID + offset as i64,
                })
                .collect::<Vec<_>>();
            let preparation = CleanerService::new(
                gateway.clone(),
                SecureJobStore::with_test_key(path.clone(), key),
            )
            .unwrap();
            let view = preparation
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: message_refs.clone(),
                })
                .await
                .unwrap();
            let plan = preparation
                .plans
                .read()
                .await
                .get(&view.id)
                .cloned()
                .unwrap();

            let completed_ids = message_refs[..COMPLETED_BATCH_SIZE]
                .iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>();
            gateway
                .delete_messages_for_everyone(CHAT_ID, &completed_ids)
                .await
                .unwrap();

            let mut job = JobRecord::new(&plan);
            job.status = JobStatus::Running;
            job.deleted = COMPLETED_BATCH_SIZE;
            job.next_batch = 1;
            let job_id = job.id;
            drop(preparation);
            SecureJobStore::with_test_key(path.clone(), key)
                .save(&PersistedState {
                    plans: vec![plan.clone()],
                    jobs: vec![job],
                })
                .unwrap();

            gateway.clear_test_traces().await;
            let service = CleanerService::new(
                gateway.clone(),
                SecureJobStore::with_test_key(path.clone(), key),
            )
            .unwrap();
            service.resume_incomplete().await;

            let finished = wait_for_terminal_job(&service, job_id).await;
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.total, MESSAGE_COUNT);
            assert_eq!(finished.deleted, MESSAGE_COUNT);
            assert_eq!(finished.skipped, 0);
            assert_eq!(finished.failed, 0);
            assert_eq!(finished.next_batch, 3);
            assert_eq!(finished.retry_after_seconds, None);
            assert_eq!(finished.error_codes, vec!["resumed_after_restart"]);
            assert_eq!(gateway.delete_batch_sizes().await, vec![100, 5]);
            assert_eq!(
                gateway.delete_calls().await,
                vec![
                    (CHAT_ID, (70_100..=70_199).collect::<Vec<_>>()),
                    (CHAT_ID, (70_200..=70_204).collect::<Vec<_>>()),
                ]
            );
            assert!(
                gateway
                    .delete_calls()
                    .await
                    .iter()
                    .flat_map(|(_, message_ids)| message_ids)
                    .all(|message_id| *message_id >= 70_100)
            );

            let reloaded = wait_for_persisted_terminal_job(&path, key, job_id).await;
            assert_eq!(reloaded.plans, vec![plan]);
            assert_eq!(reloaded.jobs.len(), 1);
            assert_eq!(reloaded.jobs[0].status, JobStatus::Completed);
            assert_eq!(reloaded.jobs[0].deleted, MESSAGE_COUNT);
            assert_eq!(reloaded.jobs[0].next_batch, 3);
            assert_eq!(reloaded.jobs[0].error_codes, vec!["resumed_after_restart"]);
        });
    }

    #[test]
    fn restart_resumes_own_message_job_from_frozen_ids() {
        tauri::async_runtime::block_on(async {
            const NEW_OWN_MESSAGE_ID: i64 = 60_001;

            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("jobs.enc");
            let key = [37; 32];
            let gateway = Arc::new(DemoGateway::new());
            let preparation = CleanerService::new(
                gateway.clone(),
                SecureJobStore::with_test_key(path.clone(), key),
            )
            .unwrap();
            let view = preparation.prepare_own_messages(-1003).await.unwrap();
            let plan = preparation
                .plans
                .read()
                .await
                .get(&view.id)
                .cloned()
                .unwrap();
            assert_eq!(plan.operation, PlanOperation::DeleteMyMessages);
            gateway.append_messages(-1003, NEW_OWN_MESSAGE_ID, 1).await;
            let mut job = JobRecord::new(&plan);
            job.status = JobStatus::Running;
            let job_id = job.id;
            drop(preparation);
            SecureJobStore::with_test_key(path.clone(), key)
                .save(&PersistedState {
                    plans: vec![plan],
                    jobs: vec![job],
                })
                .unwrap();

            gateway.clear_operation_log().await;
            let service =
                CleanerService::new(gateway.clone(), SecureJobStore::with_test_key(path, key))
                    .unwrap();
            service.resume_incomplete().await;

            let finished = wait_for_terminal_job(&service, job_id).await;
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 1);
            assert!(
                finished
                    .error_codes
                    .iter()
                    .any(|code| code == "resumed_after_restart")
            );
            assert_eq!(
                gateway.operation_log().await,
                vec!["delete_messages_for_everyone:-1003:31"]
            );
            assert!(
                gateway
                    .messages_by_ids(&[(-1003, 31)])
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                gateway
                    .messages_by_ids(&[(-1003, NEW_OWN_MESSAGE_ID)])
                    .await
                    .unwrap()
                    .len(),
                1
            );
        });
    }

    #[test]
    fn chat_scoped_plans_never_load_the_global_catalog() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [23; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();

            service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1001,
                    operation: PlanOperation::ClearHistory,
                })
                .await
                .unwrap();
            service
                .prepare_sender_action(PrepareSenderActionRequest {
                    chat_id: -1001,
                    sender_id: 714,
                })
                .await
                .unwrap();
            let own_plan = service.prepare_own_messages(-1003).await.unwrap();
            assert_eq!(own_plan.operation, PlanOperation::DeleteMyMessages);
            assert_eq!(own_plan.summary.delete_for_everyone, 1);

            assert_eq!(gateway.chat_read_counts(), (0, 3));
        });
    }

    #[test]
    fn targeted_refresh_deduplicates_sorts_and_omits_missing_chats() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [27; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();

            let snapshot = service.snapshot().await.unwrap();
            assert_eq!(snapshot.chats.len(), 8);
            let progress = service.catalog_progress();
            assert_eq!(progress.phase, "ready");
            assert_eq!((progress.processed, progress.total), (8, 8));
            let reads_after_snapshot = gateway.chat_read_counts();
            assert_eq!(reads_after_snapshot, (1, 0));

            let refreshed = service
                .refresh_chats(vec![-1001, -1001, 304])
                .await
                .unwrap();
            assert_eq!(
                refreshed
                    .iter()
                    .map(|chat| (chat.id, chat.title.as_str()))
                    .collect::<Vec<_>>(),
                vec![(-1001, "Design Team"), (304, "Empty invite")]
            );
            assert_eq!(
                gateway.chat_read_counts(),
                (reads_after_snapshot.0, reads_after_snapshot.1 + 2)
            );

            gateway.remove_chat_for_self(304).await.unwrap();
            let missing = service.refresh_chats(vec![304]).await.unwrap();
            assert!(missing.is_empty());
            assert_eq!(gateway.chat_read_counts(), (reads_after_snapshot.0, 3));
            let progress = service.catalog_progress();
            assert_eq!((progress.processed, progress.total), (7, 7));
        });
    }

    #[test]
    fn search_response_truncation_is_conservative_for_full_page() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [28; 32]);
            let gateway: Arc<dyn TelegramGateway> = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway, store).unwrap();
            let response = service
                .search(SearchRequest {
                    query: String::new(),
                    chat_ids: Vec::new(),
                    chat_kinds: Vec::new(),
                    content_kinds: Vec::new(),
                    direction: crate::model::MessageDirection::Any,
                    min_date: None,
                    max_date: None,
                    exclude_pinned: false,
                    privacy_scan: false,
                    limit: 1,
                })
                .await
                .unwrap();

            assert_eq!(response.returned, 1);
            assert_eq!(response.messages.len(), 1);
            assert!(response.truncated);
        });
    }

    #[test]
    fn admin_leave_job_deletes_every_eligible_message_before_removing_membership() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [29; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();

            let plan = service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1003,
                    operation: PlanOperation::LeaveChat,
                })
                .await
                .unwrap();
            assert_eq!(plan.operation, PlanOperation::DeleteAllMessagesAndLeave);
            assert_eq!(plan.summary.selected, 3);
            assert_eq!(plan.summary.delete_for_everyone, 1);
            assert_eq!(plan.summary.cannot_delete, 2);
            assert_eq!(
                plan.confirmation_tier,
                cleaner_domain::ConfirmationTier::High
            );

            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            gateway.clear_operation_log().await;
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: plan.chat_title,
                })
                .await
                .unwrap();

            wait_for_terminal_job(&service, job.id).await;

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 1);
            assert_eq!(finished.skipped, 2);
            assert_eq!(finished.failed, 0);
            assert!(finished.error_codes.is_empty());
            assert!(finished.retry_after_seconds.is_none());
            assert!(gateway.chat_by_id(-1003).await.unwrap().is_none());
            assert!(
                gateway
                    .messages_by_ids(&[(-1003, 31)])
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                gateway
                    .messages_by_ids(&[(-1003, 32), (-1003, 33)])
                    .await
                    .unwrap()
                    .len(),
                2
            );
            let operations = gateway.operation_log().await;
            assert_eq!(
                operations,
                vec![
                    "delete_messages_for_everyone:-1003:31",
                    "leave_chat:-1003",
                    "remove_chat_for_self:-1003",
                ]
            );
        });
    }

    #[test]
    fn leave_job_finishes_local_removal_when_membership_is_already_gone() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [30; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();
            let plan = service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1003,
                    operation: PlanOperation::LeaveChat,
                })
                .await
                .unwrap();

            gateway.leave_chat(-1003).await.unwrap();
            assert!(gateway.chat_by_id(-1003).await.unwrap().is_some());

            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: plan.chat_title,
                })
                .await
                .unwrap();

            wait_for_terminal_job(&service, job.id).await;

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            assert_eq!(finished.status, JobStatus::Completed);
            assert!(gateway.chat_by_id(-1003).await.unwrap().is_none());
        });
    }

    #[test]
    fn leave_plan_favors_whole_history_cleanup_when_telegram_allows_it() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [31; 32]);
            let gateway = Arc::new(DemoGateway::new());
            let service = CleanerService::new(gateway.clone(), store).unwrap();

            let plan = service
                .prepare_chat_action(PrepareChatActionRequest {
                    chat_id: -1002,
                    operation: PlanOperation::LeaveChat,
                })
                .await
                .unwrap();

            assert_eq!(format!("{:?}", plan.operation), "ClearHistoryAndLeave");
            assert_eq!(
                plan.confirmation_tier,
                cleaner_domain::ConfirmationTier::High
            );

            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint.clone(),
                })
                .await
                .unwrap();
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: plan.chat_title,
                })
                .await
                .unwrap();
            wait_for_terminal_job(&service, job.id).await;

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            assert_eq!(finished.status, JobStatus::Completed);
            assert!(gateway.chat_by_id(-1002).await.unwrap().is_none());
            assert!(
                gateway
                    .messages_by_ids(&[(-1002, 21), (-1002, 22), (-1002, 23), (-1002, 24)])
                    .await
                    .unwrap()
                    .is_empty()
            );
        });
    }
}
