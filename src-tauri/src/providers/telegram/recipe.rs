use std::collections::BTreeSet;

use cleaner_domain::{DeletionPlan, DeletionReach, PlanItem, PlanOperation, PlanSummary};
use retract_domain::{
    ActionKind, ActionStep, ConfirmationRequirements, ConfirmationTier, ExpectedEffect,
    RemediationPlan, RestartPolicy, Scope, VersionedPayload,
};
use serde::{Deserialize, Serialize};

use super::{
    diagnostics::invalid_recipe,
    locators::{
        TelegramActorKind, TelegramActorLocator, TelegramConversationLocator,
        TelegramMessageLocator,
    },
    normalize::{actor_locator, conversation_ref, descriptor},
};
use crate::error::AppError;

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
        let expected = bind_plan(&plan.scope, &mut legacy)?;
        if expected != *plan {
            return Err(invalid_recipe());
        }
        Ok(legacy)
    }
}

/// The sole seal: the resulting fingerprint replaces the legacy one before
/// persistence, PlanView creation, owner authentication, or execution.
pub fn bind_plan(scope: &Scope, legacy: &mut DeletionPlan) -> Result<RemediationPlan, AppError> {
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
