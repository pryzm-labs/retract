use super::*;
impl TelegramCleanup {
    pub(crate) async fn authorize_plan(
        &self,
        request: AuthorizePlanRequest,
    ) -> Result<(), AppError> {
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
        let trusted_scope = self
            .context
            .as_ref()
            .map(|context| (context.verified_user_id(), context.active().scope.source_id));
        let reason = authorization_reason(&plan, trusted_scope)?;
        if let Some(context) = &self.context {
            context
                .authenticate(&reason, self.read.info().mode == "live")
                .await?;
        } else if self.read.info().mode == "live" {
            crate::local_auth::authenticate(&reason).await?;
        }
        let mut grants = self.system_grants.lock().await;
        self.check_context()?;
        grants.issue(
            plan.id,
            plan.fingerprint,
            self.context.as_ref().map(|c| c.active().clone()),
        );
        Ok(())
    }
}

pub(super) fn authorization_reason(
    plan: &DeletionPlan,
    trusted_scope: Option<(i64, retract_domain::SourceId)>,
) -> Result<String, AppError> {
    // Context-bound live plans use the neutral envelope prefix. Context-free
    // demo and legacy fixtures retain cleaner-domain's canonical bare digest;
    // normalize only that exact 64-hex representation before extracting the
    // same 64-bit trusted-display token.
    let normalized_fingerprint = if plan.fingerprint.starts_with("sha256-v1:") {
        std::borrow::Cow::Borrowed(plan.fingerprint.as_str())
    } else {
        std::borrow::Cow::Owned(format!("sha256-v1:{}", plan.fingerprint))
    };
    let plan_token =
        retract_domain::plan_fingerprint_token(&normalized_fingerprint).map_err(|_| {
            AppError::SystemAuthentication(
                "the frozen plan has an invalid authentication fingerprint".into(),
            )
        })?;
    let target_count = frozen_target_count(plan);
    let target_word = if target_count == 1 {
        "target"
    } else {
        "targets"
    };
    let scope = trusted_scope.map_or_else(String::new, |(account_id, source_id)| {
        format!(" account {account_id}; source {};", source_id.as_uuid())
    });
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
            format!("Delete all by ‘{sender}’ ({sender_id}) in {target}")
        }
        PlanOperation::DeleteMyMessages => format!(
            "Delete {} frozen messages sent by your account in {target}",
            plan.summary.delete_for_everyone
        ),
        PlanOperation::SelectedMessages => {
            let visible_targets = plan
                .items
                .iter()
                .filter(|item| item.expected_reach == DeletionReach::Everyone)
                .take(1)
                .map(|item| format!("{}/{}", item.chat_id, item.message_id))
                .collect::<Vec<_>>();
            let remainder = plan
                .summary
                .delete_for_everyone
                .saturating_sub(visible_targets.len());
            let suffix = if remainder == 0 {
                String::new()
            } else {
                format!(" and {remainder} more")
            };
            format!(
                "Delete {} frozen Telegram messages for everyone; first target {}{suffix}",
                plan.summary.delete_for_everyone,
                visible_targets.join(", ")
            )
        }
    };
    let reason = format!(
        "Telegram plan {plan_token}:{scope} {target_count} frozen {target_word}. {action}."
    );
    if reason.chars().count() > 256
        || reason.chars().any(char::is_control)
        || reason.chars().any(is_direction_or_line_formatting)
    {
        return Err(AppError::SystemAuthentication(
            "the frozen plan cannot be represented safely for authentication".into(),
        ));
    }
    Ok(reason)
}

fn frozen_target_count(plan: &DeletionPlan) -> usize {
    match plan.operation {
        PlanOperation::SelectedMessages | PlanOperation::DeleteMyMessages => {
            plan.summary.delete_for_everyone
        }
        PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat => {
            plan.summary.delete_for_everyone.saturating_add(1)
        }
        PlanOperation::ClearHistory
        | PlanOperation::ClearHistoryAndLeave
        | PlanOperation::RemoveChatForSelf
        | PlanOperation::DeleteBySender
        | PlanOperation::DeleteGroup => 1,
    }
}

pub(super) fn trusted_prompt_label(value: &str) -> String {
    let single_line = value
        .chars()
        .filter(|character| !is_direction_or_line_formatting(*character))
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    single_line.trim().chars().take(24).collect()
}

fn is_direction_or_line_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}
