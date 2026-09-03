//! Telegram-owned normalization and the content-free bridge to the characterized
//! legacy executor. This module describes execution; it never performs deletion.
use std::collections::BTreeSet;

use cleaner_domain::{DeletionPlan, DeletionReach, PlanItem, PlanOperation, PlanSummary};
use retract_domain::{
    ActionDescriptor, ActionKind, ActionStep, Availability, BatchConstraints,
    ConfirmationRequirements, ConfirmationTier, ErrorCode, ExpectedEffect, JobCounters,
    RemediationPlan, RestartPolicy, SafeError, Scope, ScopedJobRecord, ScopedResourceRef,
    VersionedPayload,
};
use serde::{Deserialize, Serialize};

use super::locators::{
    TelegramActorKind, TelegramActorLocator, TelegramConversationLocator, TelegramMessageLocator,
};
use crate::{
    error::AppError,
    model::{JobRecord, JobStatus},
};

pub const EXECUTION_SCHEMA: &str = "telegram.compatibility_recipe";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramFrozenItem {
    pub locator: TelegramMessageLocator,
    pub reach: DeletionReach,
}

/// No inner fingerprint, body, attachment, or untyped provider metadata. Names
/// are only the encrypted names used by the existing exact confirmation flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramExecutionRecipe {
    pub operation: PlanOperation,
    pub conversation: Option<TelegramConversationLocator>,
    pub actor: Option<TelegramActorLocator>,
    pub actor_confirmation_name: Option<String>,
    pub conversation_confirmation_title: Option<String>,
    pub items: Vec<TelegramFrozenItem>,
    pub batch_size: u32,
}

impl TelegramExecutionRecipe {
    fn from_legacy(plan: &DeletionPlan) -> Result<Self, AppError> {
        Ok(Self {
            operation: plan.operation,
            conversation: plan
                .target_chat_id
                .map(|id| TelegramConversationLocator::new(id.to_string()))
                .transpose()?,
            actor: plan.target_sender_id.map(actor_locator).transpose()?,
            actor_confirmation_name: plan.target_sender_name.clone(),
            conversation_confirmation_title: plan.chat_title.clone(),
            items: plan
                .items
                .iter()
                .map(|item| {
                    Ok(TelegramFrozenItem {
                        locator: TelegramMessageLocator::new(
                            item.chat_id.to_string(),
                            item.message_id.to_string(),
                        )?,
                        reach: item.expected_reach,
                    })
                })
                .collect::<Result<_, AppError>>()?,
            batch_size: 100,
        })
    }

    pub fn from_envelope(plan: &RemediationPlan) -> Result<Self, AppError> {
        if plan.recipe.schema != EXECUTION_SCHEMA || plan.recipe.version != 1 {
            return Err(invalid_recipe());
        }
        let recipe: Self =
            serde_json::from_value(plan.recipe.payload.clone()).map_err(|_| invalid_recipe())?;
        recipe.legacy(plan)?;
        Ok(recipe)
    }

    fn legacy(&self, plan: &RemediationPlan) -> Result<DeletionPlan, AppError> {
        if self.batch_size != 100 || self.items.len() > 100_000 {
            return Err(invalid_recipe());
        }
        let target_chat_id = self
            .conversation
            .as_ref()
            .map(|locator| {
                TelegramConversationLocator::new(locator.chat_id.clone())?;
                locator.chat_id.parse::<i64>().map_err(|_| invalid_recipe())
            })
            .transpose()?;
        let target_sender_id = self
            .actor
            .as_ref()
            .map(|locator| {
                TelegramActorLocator::new(locator.kind, locator.native_id.clone())?;
                let native = locator
                    .native_id
                    .parse::<i64>()
                    .map_err(|_| invalid_recipe())?;
                if (native > 0) != (locator.kind == TelegramActorKind::User) {
                    return Err(invalid_recipe());
                }
                Ok(native)
            })
            .transpose()?;
        let mut summary = PlanSummary::default();
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for item in &self.items {
            TelegramMessageLocator::new(
                item.locator.chat_id.clone(),
                item.locator.message_id.clone(),
            )?;
            let chat_id = item.locator.chat_id.parse().map_err(|_| invalid_recipe())?;
            let message_id = item
                .locator
                .message_id
                .parse()
                .map_err(|_| invalid_recipe())?;
            if !seen.insert((chat_id, message_id))
                || (self.operation != PlanOperation::SelectedMessages
                    && Some(chat_id) != target_chat_id)
            {
                return Err(invalid_recipe());
            }
            summary.selected += 1;
            match item.reach {
                DeletionReach::Everyone => summary.delete_for_everyone += 1,
                DeletionReach::SelfOnly => summary.self_only += 1,
                DeletionReach::None => summary.cannot_delete += 1,
            }
            items.push(PlanItem {
                chat_id,
                message_id,
                expected_reach: item.reach,
            });
        }
        if items.windows(2).any(|pair| {
            (pair[0].chat_id, pair[0].message_id) >= (pair[1].chat_id, pair[1].message_id)
        }) {
            return Err(invalid_recipe());
        }
        let frozen = matches!(
            self.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::LeaveChat
                | PlanOperation::DeleteAllMessagesAndLeave
        );
        if (!frozen && !items.is_empty())
            || (self.operation == PlanOperation::SelectedMessages) != target_chat_id.is_none()
            || (self.operation == PlanOperation::DeleteBySender) != target_sender_id.is_some()
            || target_sender_id.is_some() != self.actor_confirmation_name.is_some()
            || target_chat_id.is_some() != self.conversation_confirmation_title.is_some()
            || self
                .actor_confirmation_name
                .as_ref()
                .is_some_and(|v| v.trim().is_empty() || v.chars().count() > 256)
            || (matches!(
                self.operation,
                PlanOperation::SelectedMessages | PlanOperation::DeleteMyMessages
            ) && summary.delete_for_everyone == 0)
        {
            return Err(invalid_recipe());
        }
        let tier = legacy_tier(self.operation, &summary);
        Ok(DeletionPlan {
            id: plan.id,
            operation: self.operation,
            target_chat_id,
            target_sender_id,
            target_sender_name: self.actor_confirmation_name.clone(),
            chat_title: self.conversation_confirmation_title.clone(),
            items,
            summary,
            confirmation_tier: tier,
            fingerprint: plan.fingerprint.clone(),
            created_at: plan.created_at,
        })
    }

    pub fn validate_envelope(plan: &RemediationPlan) -> Result<DeletionPlan, AppError> {
        plan.validate().map_err(|_| invalid_recipe())?;
        let mut legacy = Self::from_envelope(plan)?.legacy(plan)?;
        let expected = TelegramCompatibilityProvider::bind_plan(&plan.scope, &mut legacy)?;
        if expected != *plan {
            return Err(invalid_recipe());
        }
        Ok(legacy)
    }
}

pub struct TelegramCompatibilityProvider {
    gateway: std::sync::Arc<dyn crate::gateway::TelegramGateway>,
    context: std::sync::Arc<super::engine_context::EngineContext>,
    engine: std::sync::Arc<crate::service::CleanerService>,
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

    pub async fn search_filtered(
        &self,
        request: crate::model::SearchRequest,
    ) -> Result<Vec<retract_domain::ContentRecord>, AppError> {
        self.check_scope(&self.context.active().scope)?;
        let result = self.engine.search(request).await?;
        self.check_scope(&self.context.active().scope)?;
        result
            .messages
            .iter()
            .map(|m| normalize_content(&self.context.active().scope, m))
            .collect()
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
    /// The sole seal: the resulting fingerprint replaces the legacy one before
    /// persistence, PlanView creation, owner authentication, or execution.
    pub fn bind_plan(
        scope: &Scope,
        legacy: &mut DeletionPlan,
    ) -> Result<RemediationPlan, AppError> {
        let recipe = TelegramExecutionRecipe::from_legacy(legacy)?;
        let tier = neutral_tier(legacy.confirmation_tier);
        let mut steps = Vec::new();
        for batch in legacy.everyone_batches(100)? {
            let mut targets = batch
                .message_ids
                .iter()
                .map(|id| {
                    TelegramMessageLocator::new(batch.chat_id.to_string(), id.to_string())
                        .map(|l| l.scoped(scope.clone()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            targets.sort_by_key(|target| target.id);
            steps.push(ActionStep {
                descriptor: descriptor(
                    ActionKind::DeleteRemoteItem,
                    ExpectedEffect::RemovedForAllParticipants,
                    tier,
                ),
                targets,
            });
        }
        let conversation = legacy
            .target_chat_id
            .map(|id| conversation_ref(scope, id))
            .transpose()?;
        let mut add = |kind, effect| -> Result<(), AppError> {
            steps.push(ActionStep {
                descriptor: descriptor(kind, effect, tier),
                targets: vec![conversation.clone().ok_or_else(invalid_recipe)?],
            });
            Ok(())
        };
        match legacy.operation {
            PlanOperation::ClearHistory | PlanOperation::ClearHistoryAndLeave => add(
                ActionKind::ClearConversation,
                ExpectedEffect::RemovedForAllParticipants,
            )?,
            PlanOperation::RemoveChatForSelf => add(
                ActionKind::RemoveForCurrentAccount,
                ExpectedEffect::RemovedForCurrentAccountOnly,
            )?,
            PlanOperation::DeleteGroup => add(
                ActionKind::DeleteConversation,
                ExpectedEffect::ContainerDestroyed,
            )?,
            PlanOperation::DeleteBySender => add(
                ActionKind::DeleteByActor,
                ExpectedEffect::RemovedForAllParticipants,
            )?,
            _ => (),
        }
        if matches!(
            legacy.operation,
            PlanOperation::ClearHistoryAndLeave
                | PlanOperation::DeleteAllMessagesAndLeave
                | PlanOperation::LeaveChat
        ) {
            add(
                ActionKind::LeaveConversation,
                ExpectedEffect::MembershipRemoved,
            )?;
            add(
                ActionKind::RemoveForCurrentAccount,
                ExpectedEffect::RemovedForCurrentAccountOnly,
            )?;
        }
        let targets = steps.iter().flat_map(|step| step.targets.clone()).collect();
        let mut envelope = RemediationPlan {
            id: legacy.id,
            scope: scope.clone(),
            steps,
            targets,
            confirmation: ConfirmationRequirements {
                tier,
                acknowledgement_required: true,
                owner_auth_required: true,
                exact_text: if tier >= ConfirmationTier::High {
                    legacy.chat_title.clone()
                } else {
                    None
                },
            },
            recipe: VersionedPayload {
                schema: EXECUTION_SCHEMA.into(),
                version: 1,
                payload: serde_json::to_value(recipe).map_err(|_| invalid_recipe())?,
            },
            restart_policy: if matches!(
                legacy.operation,
                PlanOperation::SelectedMessages | PlanOperation::DeleteMyMessages
            ) {
                RestartPolicy::ResumeFrozenTargets
            } else {
                RestartPolicy::RequiresNewReview
            },
            created_at: legacy.created_at,
            fingerprint: String::new(),
        };
        envelope.seal().map_err(|_| invalid_recipe())?;
        // Validate the typed recipe independently of producer assumptions.
        let reconstructed = TelegramExecutionRecipe::from_envelope(&envelope)?.legacy(&envelope)?;
        if reconstructed.summary != legacy.summary
            || reconstructed.confirmation_tier != legacy.confirmation_tier
        {
            return Err(invalid_recipe());
        }
        legacy.fingerprint = envelope.fingerprint.clone();
        Ok(envelope)
    }

    pub fn normalize_job(
        scope: &Scope,
        plan: &DeletionPlan,
        job: &JobRecord,
        started_authorized: bool,
    ) -> Result<ScopedJobRecord, AppError> {
        let status = match job.status {
            JobStatus::Queued => retract_domain::JobStatus::Queued,
            JobStatus::Running => retract_domain::JobStatus::Running,
            JobStatus::Completed => retract_domain::JobStatus::Completed,
            JobStatus::Partial => retract_domain::JobStatus::Partial,
            JobStatus::Failed => retract_domain::JobStatus::Failed,
            JobStatus::Cancelled => retract_domain::JobStatus::Cancelled,
        };
        Ok(ScopedJobRecord {
            id: job.id,
            plan_id: job.plan_id,
            scope: scope.clone(),
            dirty_refs: job
                .target_chat_ids
                .iter()
                .map(|id| conversation_ref(scope, *id))
                .collect::<Result<_, _>>()?,
            status,
            counters: JobCounters {
                selected: plan.summary.selected as u64,
                eligible: job.total as u64,
                deleted: job.deleted as u64,
                skipped: job.skipped as u64,
                failed: job.failed as u64,
                uncertain: job.uncertain as u64,
            },
            next_batch: job.next_batch as u64,
            retry_at: job.retry_at.or_else(|| {
                job.retry_after_seconds
                    .map(|s| job.updated_at + chrono::Duration::seconds(s.min(86400) as i64))
            }),
            diagnostics: job
                .error_codes
                .iter()
                .enumerate()
                .map(|(index, code)| {
                    job.scoped_diagnostics
                        .get(index)
                        .filter(|d| legacy_diagnostic_code(d.code) == code)
                        .cloned()
                        .unwrap_or_else(|| {
                            let mut diagnostic = safe_diagnostic(code);
                            if diagnostic.code == ErrorCode::RateLimited {
                                diagnostic.retry_at = job.retry_at;
                            }
                            diagnostic
                        })
                })
                .collect(),
            started_authorized,
            created_at: job.created_at,
            updated_at: job.updated_at,
        })
    }
}

pub(crate) fn descriptor(
    kind: ActionKind,
    effect: ExpectedEffect,
    tier: ConfirmationTier,
) -> ActionDescriptor {
    ActionDescriptor {
        id: format!("telegram.{kind:?}"),
        kind,
        effect,
        availability: Availability::LivePreflightRequired,
        unavailable_reason: None,
        requires_live_preflight: true,
        batch: BatchConstraints {
            max_targets: if kind == ActionKind::DeleteRemoteItem {
                100
            } else {
                1
            },
            max_parallel: 1,
        },
        confirmation_tier: tier,
        destructive: true,
        irreversible: true,
        advisory: Some(retract_domain::ActionAdvisory {
            cost_bearing: false,
            rate_limited: true,
        }),
    }
}

pub(crate) fn conversation_ref(scope: &Scope, id: i64) -> Result<ScopedResourceRef, AppError> {
    let resource = TelegramConversationLocator::new(id.to_string())?.resource(scope.account_id);
    Ok(ScopedResourceRef {
        scope: scope.clone(),
        id: resource.resource_id().map_err(|_| invalid_recipe())?,
        resource,
    })
}

fn actor_locator(id: i64) -> Result<TelegramActorLocator, AppError> {
    TelegramActorLocator::new(
        if id > 0 {
            TelegramActorKind::User
        } else {
            TelegramActorKind::Chat
        },
        id.to_string(),
    )
}

fn legacy_tier(
    operation: PlanOperation,
    summary: &PlanSummary,
) -> cleaner_domain::ConfirmationTier {
    use cleaner_domain::ConfirmationTier::*;
    match operation {
        PlanOperation::SelectedMessages => {
            if summary.selected <= 10 {
                Low
            } else {
                Medium
            }
        }
        PlanOperation::RemoveChatForSelf => Medium,
        PlanOperation::LeaveChat if summary.delete_for_everyone == 0 => Medium,
        PlanOperation::DeleteGroup => Critical,
        _ => High,
    }
}

fn neutral_tier(tier: cleaner_domain::ConfirmationTier) -> ConfirmationTier {
    match tier {
        cleaner_domain::ConfirmationTier::Low => ConfirmationTier::Low,
        cleaner_domain::ConfirmationTier::Medium => ConfirmationTier::Medium,
        cleaner_domain::ConfirmationTier::High => ConfirmationTier::High,
        cleaner_domain::ConfirmationTier::Critical => ConfirmationTier::Critical,
    }
}

pub(crate) fn safe_diagnostic(code: &str) -> SafeError {
    SafeError {
        code: match code {
            "state_persistence_failed" | "secure_store" => ErrorCode::StatePersistenceFailed,
            "stale_context" => ErrorCode::StaleContext,
            "ambiguous_outcome" => ErrorCode::AmbiguousOutcome,
            "scope_mismatch" => ErrorCode::ScopeMismatch,
            "restart_requires_new_review" => ErrorCode::RestartRequiresNewReview,
            "not_found" => ErrorCode::NotFound,
            "telegram_timeout" => ErrorCode::Transient,
            "telegram_rate_limited" => ErrorCode::RateLimited,
            "authentication_required" | "system_authentication" => {
                ErrorCode::AuthenticationRequired
            }
            "already_removed" => ErrorCode::AlreadyRemoved,
            "cost_limit_reached" => ErrorCode::CostLimitReached,
            "permanent" | "invalid_plan" => ErrorCode::Permanent,
            "unsupported_schema" => ErrorCode::UnsupportedSchema,
            "invalid_archive" => ErrorCode::InvalidArchive,
            "unsupported_contract_version" => ErrorCode::UnsupportedContractVersion,
            "identity_unavailable" => ErrorCode::IdentityUnavailable,
            "profile_in_use" => ErrorCode::ProfileInUse,
            "migration_requires_new_review" | "legacy_store_requires_new_review" => {
                ErrorCode::MigrationRequiresNewReview
            }
            _ => ErrorCode::PermissionChanged,
        },
        retry_at: None,
    }
}

pub(crate) fn legacy_diagnostic_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::AuthenticationRequired => "authentication_required",
        ErrorCode::PermissionChanged => "telegram_rejected",
        ErrorCode::NotFound => "not_found",
        ErrorCode::AlreadyRemoved => "already_removed",
        ErrorCode::RateLimited => "telegram_rate_limited",
        ErrorCode::CostLimitReached => "cost_limit_reached",
        ErrorCode::Transient => "telegram_timeout",
        ErrorCode::Permanent => "permanent",
        ErrorCode::AmbiguousOutcome => "ambiguous_outcome",
        ErrorCode::UnsupportedSchema => "unsupported_schema",
        ErrorCode::InvalidArchive => "invalid_archive",
        ErrorCode::UnsupportedContractVersion => "unsupported_contract_version",
        ErrorCode::ScopeMismatch => "scope_mismatch",
        ErrorCode::StaleContext => "stale_context",
        ErrorCode::IdentityUnavailable => "identity_unavailable",
        ErrorCode::ProfileInUse => "profile_in_use",
        ErrorCode::StatePersistenceFailed => "state_persistence_failed",
        ErrorCode::MigrationRequiresNewReview => "migration_requires_new_review",
        ErrorCode::RestartRequiresNewReview => "restart_requires_new_review",
    }
}

pub(crate) fn invalid_recipe() -> AppError {
    AppError::InvalidRequest("invalid Telegram execution recipe".into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramContentMetadata {
    pub original_kind: cleaner_domain::ContentKind,
    pub outgoing: bool,
    pub pinned: bool,
    pub grouping: Option<ScopedResourceRef>,
    pub sender_name: String,
    pub deletion_reach: DeletionReach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramConversationMetadata {
    pub original_kind: cleaner_domain::ChatKind,
    pub archived: bool,
    pub conversation_state: cleaner_domain::ConversationState,
    pub capabilities: cleaner_domain::ChatCapabilities,
    pub avatar_seed: u8,
}

fn metadata(schema: &str, value: &impl Serialize) -> Result<VersionedPayload, AppError> {
    Ok(VersionedPayload {
        schema: schema.into(),
        version: 1,
        payload: serde_json::to_value(value).map_err(|_| invalid_recipe())?,
    })
}

pub fn normalize_conversation(
    scope: &Scope,
    chat: &cleaner_domain::ChatSummary,
) -> Result<retract_domain::ConversationRecord, AppError> {
    let resource =
        TelegramConversationLocator::new(chat.id.to_string())?.resource(scope.account_id);
    let record = retract_domain::ConversationRecord {
        id: resource
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        scope: scope.clone(),
        resource,
        kind: match chat.kind {
            cleaner_domain::ChatKind::Direct | cleaner_domain::ChatKind::Secret => {
                retract_domain::ConversationKind::Direct
            }
            cleaner_domain::ChatKind::BasicGroup | cleaner_domain::ChatKind::Supergroup => {
                retract_domain::ConversationKind::Group
            }
            cleaner_domain::ChatKind::Channel => retract_domain::ConversationKind::Broadcast,
        },
        title: chat.title.clone(),
        parent_id: None,
        participant_count: chat.member_count.map(u64::from),
        participants: Vec::new(),
        evidence: retract_domain::EvidenceState::Live,
        observed_at: chrono::Utc::now(),
        provider_metadata: Some(metadata(
            "telegram.conversation_metadata",
            &TelegramConversationMetadata {
                original_kind: chat.kind,
                archived: chat.archived,
                conversation_state: chat.conversation_state,
                capabilities: chat.capabilities.clone(),
                avatar_seed: chat.avatar_seed,
            },
        )?),
    };
    record.validate(scope).map_err(|_| invalid_recipe())?;
    Ok(record)
}

pub fn normalize_content(
    scope: &Scope,
    message: &cleaner_domain::MessageSnapshot,
) -> Result<retract_domain::ContentRecord, AppError> {
    let resource =
        TelegramMessageLocator::new(message.chat_id.to_string(), message.message_id.to_string())?
            .resource(scope.account_id);
    let actor = actor_locator(message.sender_id)?.resource(scope.account_id);
    let grouping = message
        .album_id
        .map(|id| -> Result<ScopedResourceRef, AppError> {
            let resource = super::locators::TelegramGroupingLocator::new(
                message.chat_id.to_string(),
                id.to_string(),
            )?
            .resource(scope.account_id);
            Ok(ScopedResourceRef {
                scope: scope.clone(),
                id: resource.resource_id().map_err(|_| invalid_recipe())?,
                resource,
            })
        })
        .transpose()?;
    let kind = match message.content_kind {
        cleaner_domain::ContentKind::Photo => retract_domain::ContentKind::Image,
        cleaner_domain::ContentKind::File => retract_domain::ContentKind::Document,
        other => serde_json::from_value(serde_json::to_value(other).map_err(|_| invalid_recipe())?)
            .map_err(|_| invalid_recipe())?,
    };
    let privacy_findings = message
        .privacy_findings
        .iter()
        .map(|finding| {
            serde_json::from_value(serde_json::to_value(finding).map_err(|_| invalid_recipe())?)
                .map_err(|_| invalid_recipe())
        })
        .collect::<Result<_, _>>()?;
    let record = retract_domain::ContentRecord {
        id: resource
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        scope: scope.clone(),
        conversation_id: conversation_ref(scope, message.chat_id)?
            .id
            .try_into()
            .map_err(|_| invalid_recipe())?,
        resource,
        author_id: actor
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        timestamp: message.sent_at,
        edited_at: None,
        kind,
        searchable_text: message.preview.clone(),
        attachments: Vec::new(),
        reply_to: None,
        thread_parent: None,
        external_location: retract_domain::ExternalLocationAvailability::Unsupported,
        evidence: retract_domain::EvidenceState::Live,
        observed_at: chrono::Utc::now(),
        privacy_findings,
        detector_version: if message.privacy_findings.is_empty() {
            None
        } else {
            Some("telegram-sensitive-v1".into())
        },
        provider_metadata: Some(metadata(
            "telegram.content_metadata",
            &TelegramContentMetadata {
                original_kind: message.content_kind,
                outgoing: message.is_outgoing,
                pinned: message.is_pinned,
                grouping,
                sender_name: message.sender_name.clone(),
                deletion_reach: message.deletion_reach,
            },
        )?),
    };
    record.validate(scope).map_err(|_| invalid_recipe())?;
    Ok(record)
}

#[async_trait::async_trait]
impl crate::providers::ports::QuerySource for TelegramCompatibilityProvider {
    async fn list_conversations(
        &self,
        request: crate::providers::ports::ConversationQuery,
    ) -> Result<
        crate::providers::ports::Page<retract_domain::ConversationRecord>,
        retract_domain::ProviderError,
    > {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.cursor.is_some() || request.limit == 0 || request.limit > 100_000 {
            return Err(provider_error(invalid_recipe()));
        }
        let chats = self.gateway.chats().await.map_err(provider_error)?;
        self.check_scope(&request.scope).map_err(provider_error)?;
        if chats.len() > request.limit as usize {
            return Err(provider_error(invalid_recipe()));
        }
        Ok(crate::providers::ports::Page {
            items: chats
                .iter()
                .map(|chat| normalize_conversation(&request.scope, chat))
                .collect::<Result<_, _>>()
                .map_err(provider_error)?,
            next_cursor: None,
        })
    }
    async fn search(
        &self,
        request: crate::providers::ports::ContentQuery,
    ) -> Result<
        crate::providers::ports::Page<retract_domain::ContentRecord>,
        retract_domain::ProviderError,
    > {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.cursor.is_some() {
            return Err(provider_error(invalid_recipe()));
        }
        let items = self
            .search_filtered(crate::model::SearchRequest {
                query: request.query,
                chat_ids: Vec::new(),
                chat_kinds: Vec::new(),
                content_kinds: Vec::new(),
                direction: crate::model::MessageDirection::Any,
                min_date: None,
                max_date: None,
                exclude_pinned: false,
                privacy_scan: false,
                limit: request.limit as usize,
            })
            .await
            .map_err(provider_error)?;
        Ok(crate::providers::ports::Page {
            items,
            next_cursor: None,
        })
    }
    async fn resolve(
        &self,
        request: crate::providers::ports::ResolveRequest,
    ) -> Result<Vec<retract_domain::ContentRecord>, retract_domain::ProviderError> {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.refs.len() > 100_000 {
            return Err(provider_error(invalid_recipe()));
        }
        let mut ids = Vec::new();
        for target in request.refs {
            target
                .validate(&request.scope)
                .map_err(|_| provider_error(invalid_recipe()))?;
            crate::persistence::ProviderPayloadValidator::validate_resource(
                &super::locators::TelegramPayloadValidator,
                &target.resource,
            )
            .map_err(provider_error)?;
            if target.resource.resource_kind != retract_domain::ResourceKind::Content {
                return Err(provider_error(invalid_recipe()));
            }
            let locator: TelegramMessageLocator =
                serde_json::from_value(target.resource.locator_payload)
                    .map_err(|_| provider_error(invalid_recipe()))?;
            ids.push((
                locator
                    .chat_id
                    .parse()
                    .map_err(|_| provider_error(invalid_recipe()))?,
                locator
                    .message_id
                    .parse()
                    .map_err(|_| provider_error(invalid_recipe()))?,
            ));
        }
        let messages = self
            .gateway
            .messages_by_ids(&ids)
            .await
            .map_err(provider_error)?;
        self.check_scope(&request.scope).map_err(provider_error)?;
        messages
            .iter()
            .map(|message| normalize_content(&request.scope, message).map_err(provider_error))
            .collect()
    }
}

fn provider_error(error: AppError) -> retract_domain::ProviderError {
    retract_domain::ProviderError {
        code: match error {
            AppError::NotFound => retract_domain::ProviderErrorKind::NotFound,
            AppError::Gateway(_) => retract_domain::ProviderErrorKind::PermissionChanged,
            AppError::Timeout(_) => retract_domain::ProviderErrorKind::Transient,
            _ => retract_domain::ProviderErrorKind::AuthenticationRequired,
        },
        retry_at: None,
    }
}
