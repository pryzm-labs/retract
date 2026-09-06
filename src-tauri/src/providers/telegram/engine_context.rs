//! Immutable account/session and transactional v3 projection for the existing
//! Telegram engine. A failed publication quarantines the owning engine.
use async_trait::async_trait;
use retract_domain::{ActiveContext, RemediationPlan, Scope};
use std::sync::{Arc, Mutex};

use super::{
    diagnostics::{invalid_recipe, legacy_diagnostic_code},
    identity::{SessionBinding, VerifiedTelegramIdentity},
    model::{CatalogProgress, JobRecord, JobStatus, PersistedState, SearchRequest},
    native::ports::{
        GatewayInfo, TelegramConnectionIo, TelegramMutation, TelegramRead, TelegramSession,
    },
    normalize::normalize_job,
    recipe::{TelegramExecutionRecipe, bind_plan},
};
#[cfg(test)]
use crate::secure_store::SecureJobStore;
use crate::{error::AppError, gateway::TelegramGateway, persistence::FoundationStore};

pub struct EngineContext {
    active: ActiveContext,
    identity: VerifiedTelegramIdentity,
    binding: Arc<SessionBinding>,
    quarantined: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(crate) owner_prompt: Option<Arc<dyn TestOwnerPrompt>>,
}

impl EngineContext {
    pub fn new(
        active: ActiveContext,
        identity: VerifiedTelegramIdentity,
        binding: Arc<SessionBinding>,
    ) -> Result<Self, AppError> {
        binding.validate(&active)?;
        if active.session_generation != identity.session_generation {
            return Err(stale_context());
        }
        Ok(Self {
            active,
            identity,
            binding,
            quarantined: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            owner_prompt: None,
        })
    }
    pub fn active(&self) -> &ActiveContext {
        &self.active
    }
    pub fn check(&self, gateway: &dyn TelegramSession) -> Result<(), AppError> {
        if self.quarantined.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AppError::StatePersistenceFailed);
        }
        self.binding
            .validate(&self.active)
            .map_err(|_| stale_context())?;
        if gateway.verified_identity().as_ref() != Some(&self.identity) {
            return Err(stale_context());
        }
        Ok(())
    }
    pub(crate) fn quarantine(&self) {
        self.quarantined
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn bind_plan(
        &self,
        plan: &mut cleaner_domain::DeletionPlan,
    ) -> Result<RemediationPlan, AppError> {
        self.binding
            .validate(&self.active)
            .map_err(|_| stale_context())?;
        bind_plan(&self.active.scope, plan)
    }
    pub async fn authenticate(&self, reason: &str, live: bool) -> Result<(), AppError> {
        #[cfg(test)]
        if let Some(prompt) = &self.owner_prompt {
            return prompt.authenticate().await;
        }
        if live {
            crate::local_auth::authenticate(reason).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[async_trait]
pub(crate) trait TestOwnerPrompt: Send + Sync {
    async fn authenticate(&self) -> Result<(), AppError>;
}

pub trait TelegramStateRepository: Send + Sync {
    fn scope(&self) -> Option<&Scope>;
    fn load(&self) -> Result<PersistedState, AppError>;
    fn save(&self, state: &PersistedState) -> Result<(), AppError>;
    fn envelope(&self, _id: uuid::Uuid) -> Result<RemediationPlan, AppError> {
        Err(AppError::NotFound)
    }
}

// Staged only until Task 6 removes the legacy production constructor. The
// scoped constructor cannot select or fall back to this repository.
#[cfg(test)]
pub(crate) struct LegacyTelegramRepository(pub SecureJobStore);
#[cfg(test)]
impl TelegramStateRepository for LegacyTelegramRepository {
    fn scope(&self) -> Option<&Scope> {
        None
    }
    fn load(&self) -> Result<PersistedState, AppError> {
        self.0.load()
    }
    fn save(&self, state: &PersistedState) -> Result<(), AppError> {
        self.0.save(state)
    }
}

pub struct FoundationTelegramRepository {
    shared: crate::providers::lifecycle::ScopedRepository,
    last: Mutex<Option<crate::providers::lifecycle::Projection>>,
}
impl FoundationTelegramRepository {
    pub fn new(store: Arc<FoundationStore>, scope: Scope) -> Result<Self, AppError> {
        Ok(Self {
            shared: crate::providers::lifecycle::ScopedRepository::new(store, scope)?,
            last: Mutex::new(None),
        })
    }
}
impl TelegramStateRepository for FoundationTelegramRepository {
    fn envelope(&self, id: uuid::Uuid) -> Result<RemediationPlan, AppError> {
        self.shared.envelope(id)
    }
    fn scope(&self) -> Option<&Scope> {
        Some(self.shared.scope())
    }
    fn load(&self) -> Result<PersistedState, AppError> {
        let mut last = self.last.lock().map_err(|_| AppError::StateUnavailable)?;
        let projection = self.shared.load()?;
        let mut state = PersistedState::default();
        for plan in &projection.0 {
            // Descriptive-only Task 4 payloads have no executor meaning.
            state
                .plans
                .push(TelegramExecutionRecipe::validate_envelope(plan)?);
        }
        for job in &projection.1 {
            let plan = state
                .plans
                .iter()
                .find(|p| p.id == job.plan_id)
                .ok_or_else(invalid_recipe)?;
            let envelope = projection
                .0
                .iter()
                .find(|p| p.id == job.plan_id)
                .ok_or_else(invalid_recipe)?;
            let max_cursor =
                if plan.operation == cleaner_domain::PlanOperation::ClearHistoryAndLeave {
                    1
                } else {
                    plan.everyone_batches(100)?.len()
                };
            if job.next_batch > max_cursor as u64 {
                return Err(invalid_recipe());
            }
            let mut legacy = JobRecord::new(plan);
            legacy.id = job.id;
            legacy.plan_id = job.plan_id;
            legacy.status = match job.status {
                retract_domain::JobStatus::Queued
                | retract_domain::JobStatus::Running
                | retract_domain::JobStatus::Blocked => JobStatus::Queued,
                retract_domain::JobStatus::Completed => JobStatus::Completed,
                retract_domain::JobStatus::Partial => JobStatus::Partial,
                retract_domain::JobStatus::Failed => JobStatus::Failed,
                retract_domain::JobStatus::Cancelled => JobStatus::Cancelled,
            };
            legacy.total = usize::try_from(job.counters.eligible).map_err(|_| invalid_recipe())?;
            legacy.deleted = usize::try_from(job.counters.deleted).map_err(|_| invalid_recipe())?;
            legacy.skipped = usize::try_from(job.counters.skipped).map_err(|_| invalid_recipe())?;
            legacy.failed = usize::try_from(job.counters.failed).map_err(|_| invalid_recipe())?;
            legacy.uncertain =
                usize::try_from(job.counters.uncertain).map_err(|_| invalid_recipe())?;
            legacy.next_batch = job.next_batch as usize;
            legacy.retry_at = job.retry_at;
            legacy.retry_after_seconds = job
                .retry_at
                .map(|deadline| (deadline - chrono::Utc::now()).num_seconds().max(0) as u64);
            legacy.created_at = job.created_at;
            legacy.updated_at = job.updated_at;
            legacy.error_codes = job
                .diagnostics
                .iter()
                .map(|d| legacy_diagnostic_code(d.code).into())
                .collect();
            legacy.scoped_diagnostics = job.diagnostics.clone();
            let can_resume = crate::providers::lifecycle::resumable(envelope, job);
            if !legacy.status.is_terminal() && !can_resume {
                legacy.retry_at = None;
                legacy.retry_after_seconds = None;
                legacy.status = if legacy.deleted > 0 && job.started_authorized {
                    JobStatus::Partial
                } else {
                    JobStatus::Failed
                };
                legacy
                    .error_codes
                    .push("restart_requires_new_review".into());
            }
            state.jobs.push(legacy);
        }
        if last.is_none() {
            *last = Some(projection);
        }
        Ok(state)
    }
    fn save(&self, state: &PersistedState) -> Result<(), AppError> {
        let mut last = self.last.lock().map_err(|_| AppError::StateUnavailable)?;
        let expected = last.as_ref().ok_or(AppError::StateUnavailable)?;
        if expected
            .0
            .iter()
            .any(|old| !state.plans.iter().any(|p| p.id == old.id))
            || expected
                .1
                .iter()
                .any(|old| !state.jobs.iter().any(|j| j.id == old.id))
        {
            return Err(AppError::StatePersistenceFailed);
        }
        let mut plans = Vec::new();
        for legacy in &state.plans {
            let mut rebound = legacy.clone();
            let envelope = bind_plan(self.shared.scope(), &mut rebound)?;
            if rebound != *legacy {
                return Err(invalid_recipe());
            }
            if let Some(previous) = expected.0.iter().find(|p| p.id == legacy.id)
                && *previous != envelope
            {
                return Err(invalid_recipe());
            }
            plans.push(envelope);
        }
        let mut jobs = Vec::new();
        for job in &state.jobs {
            let legacy = state
                .plans
                .iter()
                .find(|p| p.id == job.plan_id)
                .ok_or_else(invalid_recipe)?;
            let previous = expected.1.iter().find(|j| j.id == job.id);
            let authorized = previous.is_none_or(|j| j.started_authorized);
            let normalized = normalize_job(self.shared.scope(), legacy, job, authorized)?;
            if let Some(previous) = previous
                && (normalized.next_batch < previous.next_batch
                    || normalized.counters.deleted < previous.counters.deleted
                    || normalized.counters.skipped < previous.counters.skipped
                    || normalized.counters.failed < previous.counters.failed
                    || normalized.counters.uncertain < previous.counters.uncertain
                    || normalized.updated_at < previous.updated_at
                    || (previous.status.is_terminal() && !normalized.status.is_terminal()))
            {
                return Err(invalid_recipe());
            }
            jobs.push(normalized);
        }
        plans.sort_by_key(|p| p.id);
        jobs.sort_by_key(|j| j.id);
        self.shared.commit(plans.clone(), jobs.clone())?;
        *last = Some((plans, jobs));
        Ok(())
    }
}

pub(crate) fn stale_context() -> AppError {
    AppError::InvalidRequest("stale_context".into())
}

pub(crate) struct SessionGateway {
    pub inner: Arc<dyn TelegramGateway>,
    pub context: Arc<EngineContext>,
}

impl SessionGateway {
    fn mutation_result(&self, result: Result<(), AppError>) -> Result<(), AppError> {
        // tdjson::request emits these exact errors only after the native send.
        // This classification belongs only to mutation calls: a read/preflight
        // timeout has no destructive outcome, and a TDLib rejection is known.
        let response_lost = matches!(&result, Err(AppError::Gateway(code))
            if matches!(code.as_str(), "TDLIB_REQUEST_TIMEOUT" | "TDLIB_RESPONSE_CHANNEL_CLOSED"));
        if response_lost || self.context.check(self.inner.as_ref()).is_err() {
            return Err(AppError::Gateway("RETRACT_AMBIGUOUS_OUTCOME".into()));
        }
        result
    }
}

impl TelegramSession for SessionGateway {
    fn info(&self) -> GatewayInfo {
        self.inner.info()
    }
    fn auth(&self) -> super::model::AuthSnapshot {
        self.inner.auth()
    }
    fn verified_identity(&self) -> Option<VerifiedTelegramIdentity> {
        self.inner.verified_identity()
    }
    fn catalog_progress(&self) -> CatalogProgress {
        self.inner.catalog_progress()
    }
}

#[async_trait]
impl TelegramRead for SessionGateway {
    async fn chats(&self) -> Result<Vec<cleaner_domain::ChatSummary>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.chats().await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn chat_by_id(
        &self,
        chat_id: i64,
    ) -> Result<Option<cleaner_domain::ChatSummary>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.chat_by_id(chat_id).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn search(
        &self,
        request: &SearchRequest,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.search(request).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn own_messages(
        &self,
        chat_id: i64,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.own_messages(chat_id).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn chat_messages(
        &self,
        chat_id: i64,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.chat_messages(chat_id).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn messages_by_ids(
        &self,
        ids: &[(i64, i64)],
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.messages_by_ids(ids).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn sender_name(&self, sender_id: i64) -> Result<String, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.sender_name(sender_id).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<cleaner_domain::DeletionReach>, AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.current_reach(chat_id, message_id).await;
        self.context.check(self.inner.as_ref())?;
        result
    }
}

#[async_trait]
impl TelegramMutation for SessionGateway {
    async fn delete_messages_for_everyone(
        &self,
        chat_id: i64,
        message_ids: &[i64],
    ) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self
            .inner
            .delete_messages_for_everyone(chat_id, message_ids)
            .await;
        self.mutation_result(result)
    }
    async fn clear_history_for_everyone(&self, chat_id: i64) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.clear_history_for_everyone(chat_id).await;
        self.mutation_result(result)
    }
    async fn clear_history_for_everyone_keep_chat(&self, chat_id: i64) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self
            .inner
            .clear_history_for_everyone_keep_chat(chat_id)
            .await;
        self.mutation_result(result)
    }
    async fn remove_chat_for_self(&self, chat_id: i64) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.remove_chat_for_self(chat_id).await;
        self.mutation_result(result)
    }
    async fn delete_group(&self, chat_id: i64) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.delete_group(chat_id).await;
        self.mutation_result(result)
    }
    async fn leave_chat(&self, chat_id: i64) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self.inner.leave_chat(chat_id).await;
        self.mutation_result(result)
    }
    async fn delete_messages_by_sender(
        &self,
        chat_id: i64,
        sender_id: i64,
    ) -> Result<(), AppError> {
        self.context.check(self.inner.as_ref())?;
        let result = self
            .inner
            .delete_messages_by_sender(chat_id, sender_id)
            .await;
        self.mutation_result(result)
    }
}

#[async_trait]
impl TelegramConnectionIo for SessionGateway {
    async fn request_qr_auth(&self) -> Result<(), AppError> {
        self.inner.request_qr_auth().await
    }
    async fn submit_phone(&self, phone: &str) -> Result<(), AppError> {
        self.inner.submit_phone(phone).await
    }
    async fn submit_email_address(&self, email: &str) -> Result<(), AppError> {
        self.inner.submit_email_address(email).await
    }
    async fn submit_email_code(&self, code: &str) -> Result<(), AppError> {
        self.inner.submit_email_code(code).await
    }
    async fn submit_code(&self, code: &str) -> Result<(), AppError> {
        self.inner.submit_code(code).await
    }
    async fn submit_password(&self, password: &str) -> Result<(), AppError> {
        self.inner.submit_password(password).await
    }
    async fn close(&self) -> Result<(), AppError> {
        self.inner.close().await
    }
}
