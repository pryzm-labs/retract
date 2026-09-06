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
        let reason = authorization_reason(&plan);
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

pub(super) fn authorization_reason(plan: &DeletionPlan) -> String {
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

pub(super) fn trusted_prompt_label(value: &str) -> String {
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
