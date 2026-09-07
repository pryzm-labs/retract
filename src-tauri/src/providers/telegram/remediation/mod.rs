//! One Telegram cleanup owner for reviewed planning, grants and execution.
mod authorization;
mod execution;
mod planning;
mod state;
#[cfg(test)]
mod tests;

use super::diagnostics::{error_code, telegram_retry_after};
use super::{
    engine_context::{EngineContext, SessionMutation, SessionRead, TelegramStateRepository},
    model::{
        AuthorizePlanRequest, ExecuteRequest, JobRecord, JobStatus, MessageRef, PersistedState,
        PlanView, PrepareChatActionRequest, PrepareSelectionRequest, PrepareSenderActionRequest,
    },
    native::ports::{TelegramMutation, TelegramRead},
};
use crate::error::AppError;
use chrono::Utc;
use cleaner_domain::{ChatSummary, ConfirmationProof, DeletionPlan, DeletionReach, PlanOperation};
use state::push_error_once;
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
const DIRECT_CHAT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct TelegramCleanup {
    read: Arc<dyn TelegramRead>,
    mutation: Arc<dyn TelegramMutation>,
    worker_owner: Weak<Self>,
    plans: RwLock<HashMap<Uuid, DeletionPlan>>,
    jobs: RwLock<HashMap<Uuid, JobRecord>>,
    cancellation: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    system_grants: Mutex<crate::providers::lifecycle::GrantBook>,
    store: Arc<dyn TelegramStateRepository>,
    context: Option<Arc<EngineContext>>,
    transition_lock: Mutex<()>,
    persistence_failed: AtomicBool,
}

use super::{
    diagnostics::boundary_error,
    locators::{TelegramActorLocator, TelegramPayloadValidator},
    model,
    normalize::normalize_job,
    recipe::TelegramExecutionRecipe,
};
use crate::{
    provider_service::{safe, validate_refs},
    providers::ports::*,
};
use async_trait::async_trait;
use retract_domain::{ActiveContext, ErrorCode, ResourceKind, SafeError, ScopedResourceRef};

impl TelegramCleanup {
    fn active_context(&self) -> Result<&EngineContext, SafeError> {
        self.context
            .as_deref()
            .ok_or_else(|| safe(ErrorCode::StaleContext))
    }
    fn check_active(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != self.active_context()?.active() {
            return Err(safe(ErrorCode::StaleContext));
        }
        self.check_context().map_err(boundary_error)
    }
    /// Raw requests cannot consume the plan-bound grant or schedule native work.
    /// ReviewedLifecycle::start is the sole public execution entry point.
    pub async fn execute_batch(
        &self,
        _batch: ExecutionBatch,
    ) -> Result<BatchResult, retract_domain::ProviderError> {
        Err(retract_domain::ProviderError {
            code: retract_domain::ProviderErrorKind::PermissionChanged,
            retry_at: None,
        })
    }
    fn normalize_live_job(
        &self,
        job: &model::JobRecord,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        let envelope = self.reviewed_plan(job.plan_id).map_err(boundary_error)?;
        let legacy =
            TelegramExecutionRecipe::validate_envelope(&envelope).map_err(boundary_error)?;
        normalize_job(&envelope.scope, &legacy, job, true).map_err(boundary_error)
    }
}

#[async_trait]
impl ReviewedLifecycle for TelegramCleanup {
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        self.discover_intents(context, targets).await
    }
    async fn prepare(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
    ) -> Result<retract_domain::RemediationPlan, SafeError> {
        self.prepare_reviewed(context, intent).await
    }
    async fn authorize(
        &self,
        context: &ActiveContext,
        plan: ReviewedPlanRef,
    ) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.authorize_plan(model::AuthorizePlanRequest {
            plan_id: plan.plan_id,
            fingerprint: plan.fingerprint,
        })
        .await
        .map_err(boundary_error)?;
        self.check_active(context)
    }
    async fn start(
        &self,
        context: &ActiveContext,
        request: StartReviewed,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        self.check_active(context)?;
        let job = self
            .start_execution(model::ExecuteRequest {
                plan_id: request.plan_id,
                fingerprint: request.fingerprint,
                irreversible_acknowledged: request.irreversible_acknowledged,
                typed_chat_title: request.typed_chat_title,
            })
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        self.normalize_live_job(&job)
    }
    async fn jobs(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<retract_domain::ScopedJobRecord>, SafeError> {
        self.check_active(context)?;
        let jobs = self.legacy_jobs().await;
        self.check_active(context)?;
        jobs.iter().map(|j| self.normalize_live_job(j)).collect()
    }
    async fn cancel(
        &self,
        context: &ActiveContext,
        job_id: uuid::Uuid,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        self.check_active(context)?;
        let job = self.cancel_job(job_id).await.map_err(boundary_error)?;
        self.check_active(context)?;
        self.normalize_live_job(&job)
    }
    async fn recover(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.resume_incomplete().await;
        self.check_active(context)
    }
    async fn has_workers(&self) -> bool {
        self.has_workers().await
    }
    async fn stop(&self) {
        self.stop_workers().await;
    }
}
