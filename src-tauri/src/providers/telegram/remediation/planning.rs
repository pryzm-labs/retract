use super::*;
use crate::providers::telegram::{
    diagnostics::{invalid_recipe, safe_diagnostic},
    locators::{TelegramConversationLocator, TelegramMessageLocator},
    normalize::descriptor,
};
use retract_domain::{ActionKind, Availability, ConfirmationTier, ExpectedEffect, Scope};
impl TelegramCleanup {
    pub(crate) async fn prepare_selection(
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
        let snapshots = self.read.messages_by_ids(&ids).await?;
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

    pub(crate) async fn prepare_chat_action(
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
                self.read.chat_messages(chat.id).await?
            } else {
                self.read.own_messages(chat.id).await?
            };
            DeletionPlan::leave_chat(&chat, messages)?
        } else {
            DeletionPlan::chat_wide(request.operation, &chat)?
        };
        self.publish_plan(plan).await
    }

    pub(crate) async fn prepare_own_messages(&self, chat_id: i64) -> Result<PlanView, AppError> {
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
        let messages = self.read.own_messages(chat_id).await?;
        if messages.is_empty() {
            return Err(AppError::InvalidRequest(
                "Telegram found no messages sent by your account in this group".into(),
            ));
        }
        let plan = DeletionPlan::own_messages(&chat, messages)?;
        self.publish_plan(plan).await
    }

    pub(crate) async fn prepare_sender_action(
        &self,
        request: PrepareSenderActionRequest,
    ) -> Result<PlanView, AppError> {
        let chat = self
            .lookup_chat_with_timeout(request.chat_id, "sender authority check")
            .await?
            .ok_or(AppError::NotFound)?;
        let sender_name = self.read.sender_name(request.sender_id).await?;
        let plan = DeletionPlan::by_sender(&chat, request.sender_id, sender_name)?;
        self.publish_plan(plan).await
    }

    pub(super) async fn resolve_plan_chat(&self, plan: &DeletionPlan) -> Result<i64, AppError> {
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

    pub(super) async fn lookup_chat_with_timeout(
        &self,
        chat_id: i64,
        operation: &str,
    ) -> Result<Option<ChatSummary>, AppError> {
        tokio::time::timeout(DIRECT_CHAT_LOOKUP_TIMEOUT, self.read.chat_by_id(chat_id))
            .await
            .map_err(|_| {
                AppError::Timeout(format!(
                    "Telegram did not answer the {operation} within {} seconds. Try again.",
                    DIRECT_CHAT_LOOKUP_TIMEOUT.as_secs()
                ))
            })?
    }

    pub(super) fn check_scope(&self, scope: &Scope) -> Result<(), AppError> {
        if scope
            != &self
                .context
                .as_ref()
                .ok_or_else(crate::providers::telegram::engine_context::stale_context)?
                .active()
                .scope
        {
            return Err(crate::providers::telegram::engine_context::stale_context());
        }
        self.check_context()
    }

    pub(crate) async fn actions_for(
        &self,
        request: crate::providers::ports::ActionRequest,
    ) -> Result<Vec<crate::providers::ports::ActionResult>, AppError> {
        if request.context
            != *self
                .context
                .as_ref()
                .ok_or_else(crate::providers::telegram::engine_context::stale_context)?
                .active()
        {
            return Err(crate::providers::telegram::engine_context::stale_context());
        }
        self.check_scope(&request.context.scope)?;
        let mut results = Vec::new();
        for target in request.targets {
            target
                .validate(&request.context.scope)
                .map_err(|_| invalid_recipe())?;
            crate::persistence::ProviderPayloadValidator::validate_resource(
                &crate::providers::telegram::locators::TelegramPayloadValidator,
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
                    .read
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
                    .read
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

    pub(super) async fn discover_intents(
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
                .read
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
    pub(super) async fn prepare_reviewed(
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
            self.prepare_selection(model::PrepareSelectionRequest { message_refs: refs })
                .await
        } else {
            if intent.targets.len() != 1 {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let id = chat_id(&intent.targets[0])?;
            if intent.action_id == "delete_my_messages" && intent.actor.is_none() {
                self.prepare_own_messages(id).await
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
                if (sender_id > 0)
                    != (locator.kind
                        == crate::providers::telegram::locators::TelegramActorKind::User)
                {
                    return Err(safe(ErrorCode::UnsupportedSchema));
                }
                self.prepare_sender_action(model::PrepareSenderActionRequest {
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
                self.prepare_chat_action(model::PrepareChatActionRequest {
                    chat_id: id,
                    operation,
                })
                .await
            }
        }
        .map_err(boundary_error)?;
        self.check_active(context)?;
        self.reviewed_plan(view.id).map_err(boundary_error)
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
