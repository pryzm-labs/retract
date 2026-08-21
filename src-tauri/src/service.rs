use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use cleaner_domain::{
    ChatSummary, ConfirmationProof, ConfirmationTier, DeletionPlan, DeletionReach, PlanOperation,
};
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
    secure_store::SecureJobStore,
};

pub struct CleanerService {
    gateway: Arc<dyn TelegramGateway>,
    plans: RwLock<HashMap<Uuid, DeletionPlan>>,
    jobs: RwLock<HashMap<Uuid, JobRecord>>,
    cancellation: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    system_grants: Mutex<HashMap<Uuid, SystemGrant>>,
    store: SecureJobStore,
}

struct SystemGrant {
    fingerprint: String,
    expires_at: Instant,
}

impl CleanerService {
    pub fn new(
        gateway: Arc<dyn TelegramGateway>,
        store: SecureJobStore,
    ) -> Result<Arc<Self>, AppError> {
        let persisted = store.load()?;
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
                if matches!(job.status, JobStatus::Running) {
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
            store,
        }))
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
    /// complete Telegram catalog. Demo mode keeps its in-memory chat list so
    /// browser previews and safe-demo launches remain instantaneous and useful.
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
        let view = PlanView::from(&plan);
        self.plans.write().await.insert(plan.id, plan);
        self.persist().await?;
        Ok(view)
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
        let view = PlanView::from(&plan);
        self.plans.write().await.insert(plan.id, plan);
        self.persist().await?;
        Ok(view)
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
        let view = PlanView::from(&plan);
        self.plans.write().await.insert(plan.id, plan);
        self.persist().await?;
        Ok(view)
    }

    pub async fn prepare_sender_action(
        &self,
        request: PrepareSenderActionRequest,
    ) -> Result<PlanView, AppError> {
        let chat = self
            .lookup_chat_with_timeout(request.chat_id, "sender authority check")
            .await?
            .ok_or(AppError::NotFound)?;
        let plan = DeletionPlan::by_sender(&chat, request.sender_id, request.sender_name)?;
        let view = PlanView::from(&plan);
        self.plans.write().await.insert(plan.id, plan);
        self.persist().await?;
        Ok(view)
    }

    pub async fn start_execution(
        self: &Arc<Self>,
        request: ExecuteRequest,
    ) -> Result<JobRecord, AppError> {
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

        if requires_system_auth(plan.confirmation_tier) {
            let grant = self
                .system_grants
                .lock()
                .await
                .remove(&plan.id)
                .ok_or_else(|| {
                    AppError::SystemAuthentication(
                        "confirm this frozen plan with macOS immediately before execution".into(),
                    )
                })?;
            if grant.fingerprint != plan.fingerprint || grant.expires_at < Instant::now() {
                return Err(AppError::SystemAuthentication(
                    "the plan-bound authentication grant is invalid or expired".into(),
                ));
            }
        }

        let job = JobRecord::new(&plan);
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut jobs = self.jobs.write().await;
        if jobs.values().any(|existing| existing.plan_id == plan.id) {
            return Err(AppError::InvalidRequest(
                "this frozen plan has already been started".into(),
            ));
        }
        jobs.insert(job.id, job.clone());
        drop(jobs);
        self.cancellation.lock().await.insert(job.id, cancellation);
        self.persist().await?;

        let service = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            service.run_job(job.id).await;
        });
        Ok(job)
    }

    pub async fn authorize_plan(&self, request: AuthorizePlanRequest) -> Result<(), AppError> {
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
        if !requires_system_auth(plan.confirmation_tier) {
            return Ok(());
        }
        if self.gateway.info().mode == "live" {
            let reason = match plan.operation {
                PlanOperation::DeleteGroup => "permanently delete this Telegram group",
                PlanOperation::ClearHistoryAndLeave => {
                    "clear all Telegram history for everyone, leave this chat, and remove it locally"
                }
                PlanOperation::DeleteAllMessagesAndLeave => {
                    "delete every permitted Telegram message, leave this chat, and remove it locally"
                }
                PlanOperation::LeaveChat => {
                    "revoke your Telegram messages, leave this chat, and remove it locally"
                }
                PlanOperation::ClearHistory => "clear this Telegram chat history for everyone",
                PlanOperation::RemoveChatForSelf => {
                    "remove this Telegram chat history only for this account"
                }
                PlanOperation::DeleteBySender => {
                    "delete this sender's Telegram messages for everyone"
                }
                PlanOperation::DeleteMyMessages => {
                    "delete all of your Telegram messages from this group"
                }
                PlanOperation::SelectedMessages => "delete Telegram messages for everyone",
            };
            crate::local_auth::authenticate(reason).await?;
        }
        self.system_grants.lock().await.insert(
            plan.id,
            SystemGrant {
                fingerprint: plan.fingerprint,
                expires_at: Instant::now() + Duration::from_secs(60),
            },
        );
        Ok(())
    }

    pub async fn resume_incomplete(self: &Arc<Self>) {
        let ids: Vec<_> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|job| matches!(job.status, JobStatus::Queued | JobStatus::Running))
            .map(|job| job.id)
            .collect();
        for id in ids {
            self.cancellation
                .lock()
                .await
                .insert(id, Arc::new(AtomicBool::new(false)));
            let service = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                service.run_job(id).await;
            });
        }
    }

    async fn run_job(&self, job_id: Uuid) {
        if let Err(error) = self.run_job_inner(job_id).await {
            let mut jobs = self.jobs.write().await;
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
            drop(jobs);
            let _ = self.persist().await;
        }
        self.cancellation.lock().await.remove(&job_id);
    }

    async fn run_job_inner(&self, job_id: Uuid) -> Result<(), AppError> {
        let cancellation = self
            .cancellation
            .lock()
            .await
            .get(&job_id)
            .cloned()
            .ok_or(AppError::StateUnavailable)?;
        let plan_id = {
            let mut jobs = self.jobs.write().await;
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            if job.status.is_terminal() {
                return Err(AppError::JobAlreadyTerminal);
            }
            job.status = JobStatus::Running;
            job.updated_at = Utc::now();
            job.plan_id
        };
        self.persist().await?;

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
                let result = async {
                    let chat_id = if matches!(
                        operation,
                        PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat
                    ) {
                        self.resolve_leave_membership_chat(&plan).await?
                    } else {
                        self.resolve_plan_chat(&plan).await?
                    };
                    match operation {
                        PlanOperation::ClearHistory => {
                            self.gateway.clear_history_for_everyone(chat_id).await
                        }
                        PlanOperation::RemoveChatForSelf => {
                            self.gateway.remove_chat_for_self(chat_id).await
                        }
                        PlanOperation::DeleteGroup => self.gateway.delete_group(chat_id).await,
                        PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat => {
                            self.gateway.leave_chat(chat_id).await
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
                    }
                }
                .await;
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

        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
        job.status = if job.failed > 0 {
            JobStatus::Partial
        } else {
            JobStatus::Completed
        };
        job.retry_after_seconds = None;
        job.updated_at = Utc::now();
        drop(jobs);
        self.persist().await?;
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

                let mut jobs = self.jobs.write().await;
                let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                job.skipped += skipped;
                job.next_batch = index + 1;
                job.retry_after_seconds = None;
                match result {
                    Ok(()) => job.deleted += allowed.len(),
                    Err(error) => {
                        job.failed += failed_count;
                        push_error_once(job, error_code(&error));
                    }
                }
                job.updated_at = Utc::now();
                drop(jobs);
                self.persist().await?;
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
                match self
                    .gateway
                    .clear_history_for_everyone_keep_chat(chat_id)
                    .await
                {
                    Ok(()) => {
                        let mut jobs = self.jobs.write().await;
                        let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                        job.next_batch = 1;
                        job.updated_at = Utc::now();
                        drop(jobs);
                        self.persist().await?;
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
            let chat_id = self.resolve_leave_membership_chat(plan).await?;
            match self.gateway.leave_chat(chat_id).await {
                Ok(()) => return Ok(false),
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

    async fn resolve_leave_membership_chat(&self, plan: &DeletionPlan) -> Result<i64, AppError> {
        let target_chat_id = plan.target_chat_id.ok_or_else(|| {
            AppError::InvalidRequest("leave plan is missing its immutable chat ID".into())
        })?;
        let current = self
            .lookup_chat_with_timeout(target_chat_id, "execution-time membership check")
            .await?
            .ok_or(AppError::NotFound)?;
        if !current.capabilities.can_leave_chat {
            return Err(AppError::Gateway("CHAT_MEMBER_REQUIRED".into()));
        }
        Ok(target_chat_id)
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
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
        job.status = JobStatus::Cancelled;
        job.retry_after_seconds = None;
        job.updated_at = Utc::now();
        drop(jobs);
        self.persist().await
    }

    async fn wait_for_retry(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
        seconds: u64,
    ) -> Result<bool, AppError> {
        {
            let mut jobs = self.jobs.write().await;
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Queued;
            job.retry_after_seconds = Some(seconds);
            job.updated_at = Utc::now();
            push_error_once(job, "telegram_rate_limited");
        }
        self.persist().await?;

        for _ in 0..seconds {
            if cancellation.load(Ordering::Acquire) {
                self.finish_cancelled(job_id).await?;
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if cancellation.load(Ordering::Acquire) {
            self.finish_cancelled(job_id).await?;
            return Ok(true);
        }

        {
            let mut jobs = self.jobs.write().await;
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Running;
            job.retry_after_seconds = None;
            job.updated_at = Utc::now();
        }
        self.persist().await?;
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
        let mut plans: Vec<_> = self.plans.read().await.values().cloned().collect();
        plans.sort_by_key(|plan| std::cmp::Reverse(plan.created_at));
        plans.truncate(50);
        let jobs = self.jobs().await;
        self.store.save(&PersistedState { plans, jobs })
    }
}

fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Gateway(_) => "telegram_rejected",
        AppError::Timeout(_) => "telegram_timeout",
        AppError::SecureStore(_) => "secure_store",
        AppError::SystemAuthentication(_) => "system_authentication",
        AppError::NotFound => "not_found",
        AppError::JobAlreadyTerminal => "job_terminal",
        AppError::Domain(_) | AppError::InvalidRequest(_) => "invalid_plan",
        AppError::StateUnavailable => "state_unavailable",
    }
}

fn requires_system_auth(tier: ConfirmationTier) -> bool {
    matches!(tier, ConfirmationTier::High | ConfirmationTier::Critical)
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
    use crate::{demo_gateway::DemoGateway, secure_store::SecureJobStore};
    use cleaner_domain::PlanOperation;

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

            for _ in 0..50 {
                if service
                    .jobs()
                    .await
                    .iter()
                    .find(|candidate| candidate.id == job.id)
                    .is_some_and(|candidate| candidate.status.is_terminal())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(matches!(
                service.start_execution(execution()).await,
                Err(AppError::SystemAuthentication(_))
            ));
            assert_eq!(service.jobs().await.len(), 1);
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
                    sender_id: 9003,
                    sender_name: "Priya".into(),
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
            let job = service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: plan.chat_title,
                })
                .await
                .unwrap();

            for _ in 0..50 {
                if service
                    .jobs()
                    .await
                    .iter()
                    .find(|candidate| candidate.id == job.id)
                    .is_some_and(|candidate| candidate.status.is_terminal())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            let finished = service
                .jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap();
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.deleted, 1);
            assert_eq!(finished.skipped, 2);
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
            for _ in 0..50 {
                if service
                    .jobs()
                    .await
                    .iter()
                    .find(|candidate| candidate.id == job.id)
                    .is_some_and(|candidate| candidate.status.is_terminal())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

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
