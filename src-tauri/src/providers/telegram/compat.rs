//! Telegram-owned compatibility provider for the characterized legacy executor.
//! Pure recipe, normalization, and diagnostic code lives in sibling modules.
use cleaner_domain::{DeletionPlan, DeletionReach};
use retract_domain::{
    ActionKind, Availability, ConfirmationTier, ExpectedEffect, RemediationPlan, Scope,
    ScopedJobRecord,
};

use super::{
    diagnostics::{invalid_recipe, safe_diagnostic},
    locators::{TelegramConversationLocator, TelegramMessageLocator},
    model::JobRecord,
    normalize::descriptor,
};
use crate::error::AppError;

pub use super::recipe::{EXECUTION_SCHEMA, TelegramExecutionRecipe, TelegramFrozenItem};

#[derive(Clone)]
pub struct TelegramCompatibilityProvider {
    pub(super) gateway: std::sync::Arc<dyn crate::gateway::TelegramGateway>,
    pub(super) context: std::sync::Arc<super::engine_context::EngineContext>,
    pub(super) engine: std::sync::Arc<crate::service::CleanerService>,
}

impl TelegramCompatibilityProvider {
    pub fn new(
        gateway: std::sync::Arc<dyn crate::gateway::TelegramGateway>,
        context: std::sync::Arc<super::engine_context::EngineContext>,
        engine: std::sync::Arc<crate::service::CleanerService>,
    ) -> Result<Self, AppError> {
        context.check(gateway.as_ref())?;
        if !engine.is_bound_to(&context) {
            return Err(super::engine_context::stale_context());
        }
        Ok(Self {
            gateway,
            context,
            engine,
        })
    }
    pub fn engine(&self) -> &std::sync::Arc<crate::service::CleanerService> {
        &self.engine
    }
    fn check_scope(&self, scope: &Scope) -> Result<(), AppError> {
        if scope != &self.context.active().scope {
            return Err(super::engine_context::stale_context());
        }
        self.context.check(self.gateway.as_ref())
    }

    pub async fn actions_for(
        &self,
        request: crate::providers::ports::ActionRequest,
    ) -> Result<Vec<crate::providers::ports::ActionResult>, AppError> {
        if request.context != *self.context.active() {
            return Err(super::engine_context::stale_context());
        }
        self.check_scope(&request.context.scope)?;
        let mut results = Vec::new();
        for target in request.targets {
            target
                .validate(&request.context.scope)
                .map_err(|_| invalid_recipe())?;
            crate::persistence::ProviderPayloadValidator::validate_resource(
                &super::locators::TelegramPayloadValidator,
                &target.resource,
            )?;
            self.check_scope(&request.context.scope)?;
            let descriptors = if target.resource.resource_kind
                == retract_domain::ResourceKind::Content
            {
                let locator: TelegramMessageLocator =
                    serde_json::from_value(target.resource.locator_payload.clone())
                        .map_err(|_| invalid_recipe())?;
                let reach = self
                    .gateway
                    .current_reach(
                        locator.chat_id.parse().map_err(|_| invalid_recipe())?,
                        locator.message_id.parse().map_err(|_| invalid_recipe())?,
                    )
                    .await?;
                let mut action = descriptor(
                    ActionKind::DeleteRemoteItem,
                    ExpectedEffect::RemovedForAllParticipants,
                    ConfirmationTier::Low,
                );
                if reach != Some(DeletionReach::Everyone) {
                    action.availability = Availability::Unavailable;
                    action.unavailable_reason = Some(safe_diagnostic("telegram_rejected"));
                }
                vec![action]
            } else if target.resource.resource_kind == retract_domain::ResourceKind::Conversation {
                let locator: TelegramConversationLocator =
                    serde_json::from_value(target.resource.locator_payload.clone())
                        .map_err(|_| invalid_recipe())?;
                let chat = self
                    .gateway
                    .chat_by_id(locator.chat_id.parse().map_err(|_| invalid_recipe())?)
                    .await?
                    .ok_or(AppError::NotFound)?;
                let c = &chat.capabilities;
                [
                    (
                        ActionKind::ClearConversation,
                        ExpectedEffect::RemovedForAllParticipants,
                        ConfirmationTier::High,
                        c.can_clear_for_everyone,
                    ),
                    (
                        ActionKind::RemoveForCurrentAccount,
                        ExpectedEffect::RemovedForCurrentAccountOnly,
                        ConfirmationTier::Medium,
                        c.can_remove_for_self,
                    ),
                    (
                        ActionKind::LeaveConversation,
                        ExpectedEffect::MembershipRemoved,
                        ConfirmationTier::High,
                        c.can_leave_chat,
                    ),
                    (
                        ActionKind::DeleteConversation,
                        ExpectedEffect::ContainerDestroyed,
                        ConfirmationTier::Critical,
                        c.can_delete_group,
                    ),
                    (
                        ActionKind::DeleteByActor,
                        ExpectedEffect::RemovedForAllParticipants,
                        ConfirmationTier::High,
                        c.can_delete_by_sender,
                    ),
                ]
                .into_iter()
                .map(|(kind, effect, tier, allowed)| {
                    let mut action = descriptor(kind, effect, tier);
                    if !allowed {
                        action.availability = Availability::Unavailable;
                        action.unavailable_reason = Some(safe_diagnostic("telegram_rejected"));
                    }
                    action
                })
                .collect()
            } else {
                return Err(invalid_recipe());
            };
            self.check_scope(&request.context.scope)?;
            results.push(crate::providers::ports::ActionResult {
                target,
                descriptors,
            });
        }
        Ok(results)
    }

    /// There is intentionally no ungranted native batch path. Task 6's neutral
    /// lifecycle dispatch must use the reviewed engine's single owner grant.
    pub async fn execute_batch(
        &self,
        _batch: crate::providers::ports::ExecutionBatch,
    ) -> Result<crate::providers::ports::BatchResult, retract_domain::ProviderError> {
        Err(retract_domain::ProviderError {
            code: retract_domain::ProviderErrorKind::PermissionChanged,
            retry_at: None,
        })
    }

    pub fn bind_plan(
        scope: &Scope,
        legacy: &mut DeletionPlan,
    ) -> Result<RemediationPlan, AppError> {
        super::recipe::bind_plan(scope, legacy)
    }

    pub fn normalize_job(
        scope: &Scope,
        plan: &DeletionPlan,
        job: &JobRecord,
        started_authorized: bool,
    ) -> Result<ScopedJobRecord, AppError> {
        super::normalize::normalize_job(scope, plan, job, started_authorized)
    }
}
