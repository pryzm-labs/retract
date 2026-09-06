//! Reviewed application cleanup operations for the Telegram bridge.
use super::{
    compat::TelegramCompatibilityProvider,
    diagnostics::boundary_error,
    locators::{
        TelegramActorLocator, TelegramConversationLocator, TelegramMessageLocator,
        TelegramPayloadValidator,
    },
    model,
    normalize::{descriptor, normalize_job},
    recipe::TelegramExecutionRecipe,
};
use crate::{
    provider_service::{safe, validate_refs},
    providers::ports::*,
};
use async_trait::async_trait;
use retract_domain::{ActiveContext, ErrorCode, ResourceKind, SafeError, ScopedResourceRef};

impl TelegramCompatibilityProvider {
    fn check_active(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != self.context.active() {
            return Err(safe(ErrorCode::StaleContext));
        }
        self.context
            .check(self.gateway.as_ref())
            .map_err(boundary_error)
    }
    fn normalize_live_job(
        &self,
        job: &model::JobRecord,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        let envelope = self
            .engine
            .reviewed_plan(job.plan_id)
            .map_err(boundary_error)?;
        let legacy =
            TelegramExecutionRecipe::validate_envelope(&envelope).map_err(boundary_error)?;
        normalize_job(&envelope.scope, &legacy, job, true).map_err(boundary_error)
    }
}

fn chat_id(reference: &ScopedResourceRef) -> Result<i64, SafeError> {
    if reference.resource.resource_kind != ResourceKind::Conversation {
        return Err(safe(ErrorCode::ScopeMismatch));
    }
    let locator: TelegramConversationLocator =
        serde_json::from_value(reference.resource.locator_payload.clone())
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
    locator
        .chat_id
        .parse()
        .map_err(|_| safe(ErrorCode::UnsupportedSchema))
}

#[async_trait]
impl ReviewedLifecycle for TelegramCompatibilityProvider {
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        self.check_active(context)?;
        validate_refs(context, &targets, &TelegramPayloadValidator, None)?;
        if targets.is_empty()
            || (targets
                .iter()
                .any(|t| t.resource.resource_kind != ResourceKind::Content)
                && (targets.len() != 1
                    || targets[0].resource.resource_kind != ResourceKind::Conversation))
        {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        let actions = self
            .actions_for(ActionRequest {
                context: context.clone(),
                targets: targets.clone(),
            })
            .await
            .map_err(boundary_error)?;
        let descriptors = actions
            .into_iter()
            .flat_map(|a| a.descriptors)
            .collect::<Vec<_>>();
        // Discovery describes the existing leave preparation policy without
        // enumerating history or freezing targets. The final plan remains the
        // authority for whether any eligible message-cleanup steps exist.
        let mut own_messages_supported = false;
        let leave_description = if targets.len() == 1
            && targets[0].resource.resource_kind == ResourceKind::Conversation
        {
            use retract_domain::{ActionKind, ConfirmationTier, ExpectedEffect};
            let chat = self
                .gateway
                .chat_by_id(chat_id(&targets[0])?)
                .await
                .map_err(boundary_error)?
                .ok_or_else(|| safe(ErrorCode::NotFound))?;
            self.check_active(context)?;
            let c = &chat.capabilities;
            own_messages_supported = matches!(
                chat.kind,
                cleaner_domain::ChatKind::BasicGroup | cleaner_domain::ChatKind::Supergroup
            ) && (c.role != cleaner_domain::ChatRole::Member
                || c.can_leave_chat);
            let (cleanup_kind, label) = if c.can_clear_for_everyone {
                (ActionKind::ClearConversation, "Clear all history & leave")
            } else if c.can_delete_others {
                (
                    ActionKind::DeleteRemoteItem,
                    "Delete eligible messages from all participants, if any, & leave",
                )
            } else {
                (
                    ActionKind::DeleteRemoteItem,
                    "Revoke my eligible messages, if any, & leave",
                )
            };
            let ordered = [
                (cleanup_kind, ExpectedEffect::RemovedForAllParticipants),
                (
                    ActionKind::LeaveConversation,
                    ExpectedEffect::MembershipRemoved,
                ),
                (
                    ActionKind::RemoveForCurrentAccount,
                    ExpectedEffect::RemovedForCurrentAccountOnly,
                ),
            ]
            .into_iter()
            .map(|(kind, effect)| {
                let mut action = descriptor(kind, effect, ConfirmationTier::High);
                if !c.can_leave_chat {
                    action.availability = retract_domain::Availability::Unavailable;
                    action.unavailable_reason = Some(safe(ErrorCode::PermissionChanged));
                }
                action
            })
            .collect::<Vec<_>>();
            Some((label, ordered))
        } else {
            None
        };
        let ids: &[(&str, &str, bool)] = if targets
            .iter()
            .all(|t| t.resource.resource_kind == ResourceKind::Content)
        {
            &[(
                "selected_messages",
                "Delete selected messages for everyone",
                false,
            )]
        } else {
            &[
                (
                    "delete_my_messages",
                    "Delete my messages for everyone",
                    false,
                ),
                ("clear_history", "Clear history for everyone", false),
                ("remove_chat_for_self", "Remove chat for me", false),
                ("leave_chat", "Clean up and leave chat", false),
                ("delete_by_sender", "Delete messages by sender", true),
                ("delete_group", "Permanently delete group", false),
            ]
        };
        self.check_active(context)?;
        Ok(ids
            .iter()
            .map(|(id, label, actor)| {
                use retract_domain::{ActionKind, ConfirmationTier, ExpectedEffect};
                if *id == "leave_chat"
                    && let Some((label, descriptors)) = &leave_description
                {
                    return IntentDescriptor {
                        action_id: (*id).into(),
                        label: (*label).into(),
                        requires_actor: false,
                        descriptors: descriptors.clone(),
                    };
                }
                let kind = match *id {
                    "selected_messages" | "delete_my_messages" => ActionKind::DeleteRemoteItem,
                    "clear_history" => ActionKind::ClearConversation,
                    "remove_chat_for_self" => ActionKind::RemoveForCurrentAccount,
                    "leave_chat" => ActionKind::LeaveConversation,
                    "delete_by_sender" => ActionKind::DeleteByActor,
                    "delete_group" => ActionKind::DeleteConversation,
                    _ => unreachable!("closed intent catalog"),
                };
                let descriptors = if *id == "delete_my_messages" {
                    let mut descriptor = descriptor(
                        kind,
                        ExpectedEffect::RemovedForAllParticipants,
                        ConfirmationTier::High,
                    );
                    if !own_messages_supported {
                        descriptor.availability = retract_domain::Availability::Unavailable;
                        descriptor.unavailable_reason = Some(safe(ErrorCode::PermissionChanged));
                    }
                    vec![descriptor]
                } else {
                    descriptors
                        .iter()
                        .filter(|d| d.kind == kind)
                        .cloned()
                        .collect()
                };
                IntentDescriptor {
                    action_id: (*id).into(),
                    label: (*label).into(),
                    requires_actor: *actor,
                    descriptors,
                }
            })
            .collect())
    }
    async fn prepare(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
    ) -> Result<retract_domain::RemediationPlan, SafeError> {
        self.check_active(context)?;
        validate_refs(context, &intent.targets, &TelegramPayloadValidator, None)?;
        let view = if intent.action_id == "selected_messages" {
            if intent.actor.is_some() {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let mut refs = Vec::new();
            for reference in &intent.targets {
                if reference.resource.resource_kind != ResourceKind::Content {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let locator: TelegramMessageLocator =
                    serde_json::from_value(reference.resource.locator_payload.clone())
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                refs.push(model::MessageRef {
                    chat_id: locator
                        .chat_id
                        .parse()
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
                    message_id: locator
                        .message_id
                        .parse()
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
                });
            }
            self.engine
                .prepare_selection(model::PrepareSelectionRequest { message_refs: refs })
                .await
        } else {
            if intent.targets.len() != 1 {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let id = chat_id(&intent.targets[0])?;
            if intent.action_id == "delete_my_messages" && intent.actor.is_none() {
                self.engine.prepare_own_messages(id).await
            } else if intent.action_id == "delete_by_sender" {
                let actor = intent.actor.ok_or_else(|| safe(ErrorCode::ScopeMismatch))?;
                validate_refs(
                    context,
                    std::slice::from_ref(&actor),
                    &TelegramPayloadValidator,
                    Some(ResourceKind::Actor),
                )?;
                let locator: TelegramActorLocator =
                    serde_json::from_value(actor.resource.locator_payload)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                let sender_id: i64 = locator
                    .native_id
                    .parse()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                // The reviewed legacy recipe encodes positive User and negative
                // Chat IDs. Reject a valid locator it cannot preserve rather than
                // silently changing its tagged identity during plan binding.
                if (sender_id > 0) != (locator.kind == super::locators::TelegramActorKind::User) {
                    return Err(safe(ErrorCode::UnsupportedSchema));
                }
                self.engine
                    .prepare_sender_action(model::PrepareSenderActionRequest {
                        chat_id: id,
                        sender_id,
                    })
                    .await
            } else {
                if intent.actor.is_some() {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let operation = match intent.action_id.as_str() {
                    "clear_history" => cleaner_domain::PlanOperation::ClearHistory,
                    "remove_chat_for_self" => cleaner_domain::PlanOperation::RemoveChatForSelf,
                    "leave_chat" => cleaner_domain::PlanOperation::LeaveChat,
                    "delete_group" => cleaner_domain::PlanOperation::DeleteGroup,
                    _ => return Err(safe(ErrorCode::UnsupportedSchema)),
                };
                self.engine
                    .prepare_chat_action(model::PrepareChatActionRequest {
                        chat_id: id,
                        operation,
                    })
                    .await
            }
        }
        .map_err(boundary_error)?;
        self.check_active(context)?;
        self.engine.reviewed_plan(view.id).map_err(boundary_error)
    }
    async fn authorize(
        &self,
        context: &ActiveContext,
        plan: ReviewedPlanRef,
    ) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.engine
            .authorize_plan(model::AuthorizePlanRequest {
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
            .engine
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
        let jobs = self.engine.jobs().await;
        self.check_active(context)?;
        jobs.iter().map(|j| self.normalize_live_job(j)).collect()
    }
    async fn cancel(
        &self,
        context: &ActiveContext,
        job_id: uuid::Uuid,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        self.check_active(context)?;
        let job = self
            .engine
            .cancel_job(job_id)
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        self.normalize_live_job(&job)
    }
    async fn recover(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.engine.resume_incomplete().await;
        self.check_active(context)
    }
    async fn has_workers(&self) -> bool {
        self.engine.has_workers().await
    }
    async fn stop(&self) {
        self.engine.stop_workers().await;
    }
}
