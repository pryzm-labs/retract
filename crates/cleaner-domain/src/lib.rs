#![forbid(unsafe_code)]

mod sensitive;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub use sensitive::detect_sensitive_data;

const MAX_PLAN_MESSAGES: usize = 100_000;

#[derive(Default)]
struct PlanTarget {
    chat_id: Option<i64>,
    sender_id: Option<i64>,
    sender_name: Option<String>,
    chat_title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    Direct,
    BasicGroup,
    Supergroup,
    Channel,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    Owner,
    AdminWithDelete,
    AdminLimited,
    Member,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationState {
    Empty,
    NeverReplied,
    AwaitingReply,
    Active,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionReach {
    Everyone,
    SelfOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Photo,
    Video,
    File,
    Voice,
    Audio,
    Animation,
    Sticker,
    Poll,
    Location,
    Contact,
    Service,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveDataKind {
    EmailAddress,
    PhoneNumber,
    PostalAddress,
    PreciseLocation,
    PersonalIdentifier,
    IdentityDocument,
    FinancialAccount,
    CryptoWallet,
    CredentialOrSecret,
    NetworkAddress,
    ContactCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCapabilities {
    pub role: ChatRole,
    pub can_delete_others: bool,
    pub can_clear_for_everyone: bool,
    #[serde(default)]
    pub can_remove_for_self: bool,
    pub can_delete_group: bool,
    pub can_delete_by_sender: bool,
    #[serde(default)]
    pub can_leave_chat: bool,
}

impl ChatCapabilities {
    pub fn validate(&self, kind: ChatKind) -> Result<(), DomainError> {
        if self.can_delete_group && self.role != ChatRole::Owner {
            return Err(DomainError::InconsistentCapability(
                "only an owner may be represented as able to delete a group",
            ));
        }
        if self.can_delete_by_sender && (kind != ChatKind::Supergroup || !self.can_delete_others) {
            return Err(DomainError::InconsistentCapability(
                "delete-by-sender requires supergroup delete rights",
            ));
        }
        if self.can_leave_chat
            && (!matches!(
                kind,
                ChatKind::BasicGroup | ChatKind::Supergroup | ChatKind::Channel
            ) || self.role == ChatRole::Owner)
        {
            return Err(DomainError::InconsistentCapability(
                "only non-owner members of groups or channels may leave a chat",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    pub archived: bool,
    pub member_count: Option<u32>,
    #[serde(default)]
    pub conversation_state: ConversationState,
    pub capabilities: ChatCapabilities,
    pub avatar_seed: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSnapshot {
    pub chat_id: i64,
    pub message_id: i64,
    pub sender_id: i64,
    pub sender_name: String,
    pub sent_at: DateTime<Utc>,
    pub is_outgoing: bool,
    pub content_kind: ContentKind,
    pub preview: String,
    #[serde(default)]
    pub privacy_findings: Vec<SensitiveDataKind>,
    pub album_id: Option<i64>,
    pub is_pinned: bool,
    pub deletion_reach: DeletionReach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanOperation {
    SelectedMessages,
    DeleteMyMessages,
    ClearHistory,
    ClearHistoryAndLeave,
    DeleteAllMessagesAndLeave,
    RemoveChatForSelf,
    DeleteBySender,
    DeleteGroup,
    LeaveChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationTier {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub chat_id: i64,
    pub message_id: i64,
    pub expected_reach: DeletionReach,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    pub selected: usize,
    pub delete_for_everyone: usize,
    pub self_only: usize,
    pub cannot_delete: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionPlan {
    pub id: Uuid,
    pub operation: PlanOperation,
    pub target_chat_id: Option<i64>,
    #[serde(default)]
    pub target_sender_id: Option<i64>,
    #[serde(default)]
    pub target_sender_name: Option<String>,
    pub chat_title: Option<String>,
    pub items: Vec<PlanItem>,
    pub summary: PlanSummary,
    pub confirmation_tier: ConfirmationTier,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
}

impl DeletionPlan {
    pub fn selected_messages(messages: Vec<MessageSnapshot>) -> Result<Self, DomainError> {
        let (items, summary) = plan_items(messages)?;
        let confirmation_tier = if summary.selected <= 10 {
            ConfirmationTier::Low
        } else {
            ConfirmationTier::Medium
        };

        Ok(Self::finish(
            PlanOperation::SelectedMessages,
            PlanTarget::default(),
            items,
            summary,
            confirmation_tier,
        ))
    }

    pub fn own_messages(
        chat: &ChatSummary,
        messages: Vec<MessageSnapshot>,
    ) -> Result<Self, DomainError> {
        let active_group = matches!(chat.kind, ChatKind::BasicGroup | ChatKind::Supergroup)
            && (chat.capabilities.role != ChatRole::Member || chat.capabilities.can_leave_chat);
        if !active_group {
            return Err(DomainError::OperationNotAllowed(
                PlanOperation::DeleteMyMessages,
            ));
        }
        if messages
            .iter()
            .any(|message| message.chat_id != chat.id || !message.is_outgoing)
        {
            return Err(DomainError::InvalidOwnHistoryScope);
        }
        let (items, summary) = plan_items(messages)?;
        Ok(Self::finish(
            PlanOperation::DeleteMyMessages,
            PlanTarget {
                chat_id: Some(chat.id),
                chat_title: Some(chat.title.clone()),
                ..PlanTarget::default()
            },
            items,
            summary,
            ConfirmationTier::High,
        ))
    }

    /// Freeze the broadest leave cleanup authorized by Telegram: whole-history
    /// revocation, every enumerated admin-deletable message, or the current
    /// account's own messages. Cleanup always precedes membership removal.
    pub fn leave_chat(
        chat: &ChatSummary,
        messages: Vec<MessageSnapshot>,
    ) -> Result<Self, DomainError> {
        chat.capabilities.validate(chat.kind)?;
        if !chat.capabilities.can_leave_chat {
            return Err(DomainError::OperationNotAllowed(PlanOperation::LeaveChat));
        }
        let operation = if chat.capabilities.can_clear_for_everyone {
            PlanOperation::ClearHistoryAndLeave
        } else if chat.capabilities.can_delete_others {
            PlanOperation::DeleteAllMessagesAndLeave
        } else {
            PlanOperation::LeaveChat
        };
        if messages.iter().any(|message| {
            message.chat_id != chat.id
                || (operation == PlanOperation::LeaveChat && !message.is_outgoing)
        }) {
            return Err(DomainError::InvalidOwnHistoryScope);
        }

        let (items, summary) = if operation == PlanOperation::ClearHistoryAndLeave {
            (Vec::new(), PlanSummary::default())
        } else {
            collect_plan_items(messages)?
        };
        let confirmation_tier = if matches!(
            operation,
            PlanOperation::ClearHistoryAndLeave | PlanOperation::DeleteAllMessagesAndLeave
        ) || summary.delete_for_everyone > 0
        {
            ConfirmationTier::High
        } else {
            ConfirmationTier::Medium
        };
        Ok(Self::finish(
            operation,
            PlanTarget {
                chat_id: Some(chat.id),
                chat_title: Some(chat.title.clone()),
                ..PlanTarget::default()
            },
            items,
            summary,
            confirmation_tier,
        ))
    }

    pub fn chat_wide(operation: PlanOperation, chat: &ChatSummary) -> Result<Self, DomainError> {
        chat.capabilities.validate(chat.kind)?;
        let allowed = match operation {
            PlanOperation::ClearHistory => chat.capabilities.can_clear_for_everyone,
            PlanOperation::ClearHistoryAndLeave | PlanOperation::DeleteAllMessagesAndLeave => false,
            PlanOperation::RemoveChatForSelf => chat.capabilities.can_remove_for_self,
            PlanOperation::DeleteGroup => chat.capabilities.can_delete_group,
            // Leaving uses its dedicated constructor so the frozen plan also
            // contains the account's revocable outgoing messages.
            PlanOperation::LeaveChat => false,
            PlanOperation::DeleteMyMessages => false,
            PlanOperation::DeleteBySender => false,
            PlanOperation::SelectedMessages => false,
        };
        if !allowed {
            return Err(DomainError::OperationNotAllowed(operation));
        }

        let tier = match operation {
            PlanOperation::DeleteGroup => ConfirmationTier::Critical,
            PlanOperation::ClearHistoryAndLeave | PlanOperation::DeleteAllMessagesAndLeave => {
                ConfirmationTier::High
            }
            PlanOperation::LeaveChat | PlanOperation::RemoveChatForSelf => ConfirmationTier::Medium,
            PlanOperation::DeleteMyMessages => ConfirmationTier::High,
            _ => ConfirmationTier::High,
        };
        let summary = PlanSummary {
            selected: 0,
            delete_for_everyone: 0,
            self_only: 0,
            cannot_delete: 0,
        };
        Ok(Self::finish(
            operation,
            PlanTarget {
                chat_id: Some(chat.id),
                chat_title: Some(chat.title.clone()),
                ..PlanTarget::default()
            },
            Vec::new(),
            summary,
            tier,
        ))
    }

    pub fn by_sender(
        chat: &ChatSummary,
        sender_id: i64,
        sender_name: String,
    ) -> Result<Self, DomainError> {
        chat.capabilities.validate(chat.kind)?;
        if !chat.capabilities.can_delete_by_sender {
            return Err(DomainError::OperationNotAllowed(
                PlanOperation::DeleteBySender,
            ));
        }
        validate_chat_identifier(sender_id)?;
        if sender_name.trim().is_empty() || sender_name.chars().count() > 256 {
            return Err(DomainError::InvalidSenderName);
        }
        Ok(Self::finish(
            PlanOperation::DeleteBySender,
            PlanTarget {
                chat_id: Some(chat.id),
                sender_id: Some(sender_id),
                sender_name: Some(sender_name),
                chat_title: Some(chat.title.clone()),
            },
            Vec::new(),
            PlanSummary::default(),
            ConfirmationTier::High,
        ))
    }

    fn finish(
        operation: PlanOperation,
        target: PlanTarget,
        mut items: Vec<PlanItem>,
        summary: PlanSummary,
        confirmation_tier: ConfirmationTier,
    ) -> Self {
        items.sort_by_key(|item| (item.chat_id, item.message_id));
        let fingerprint = fingerprint(
            operation,
            target.chat_id,
            target.sender_id,
            &items,
            target.chat_title.as_deref(),
        );
        Self {
            id: Uuid::new_v4(),
            operation,
            target_chat_id: target.chat_id,
            target_sender_id: target.sender_id,
            target_sender_name: target.sender_name,
            chat_title: target.chat_title,
            items,
            summary,
            confirmation_tier,
            fingerprint,
            created_at: Utc::now(),
        }
    }

    pub fn everyone_batches(&self, batch_size: usize) -> Result<Vec<DeletionBatch>, DomainError> {
        if batch_size == 0 || batch_size > 100 {
            return Err(DomainError::InvalidBatchSize(batch_size));
        }
        let mut by_chat: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for item in self
            .items
            .iter()
            .filter(|item| item.expected_reach == DeletionReach::Everyone)
        {
            by_chat
                .entry(item.chat_id)
                .or_default()
                .push(item.message_id);
        }

        let mut batches = Vec::new();
        for (chat_id, message_ids) in by_chat {
            for chunk in message_ids.chunks(batch_size) {
                batches.push(DeletionBatch {
                    chat_id,
                    message_ids: chunk.to_vec(),
                });
            }
        }
        Ok(batches)
    }

    pub fn verify_confirmation(&self, proof: &ConfirmationProof) -> Result<(), DomainError> {
        if proof.fingerprint != self.fingerprint || !proof.irreversible_acknowledged {
            return Err(DomainError::ConfirmationRejected);
        }
        match self.confirmation_tier {
            ConfirmationTier::Low | ConfirmationTier::Medium => Ok(()),
            ConfirmationTier::High | ConfirmationTier::Critical => {
                let expected = self.chat_title.as_deref().unwrap_or_default();
                if proof.typed_chat_title.as_deref() != Some(expected) {
                    return Err(DomainError::ConfirmationRejected);
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionBatch {
    pub chat_id: i64,
    pub message_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmationProof {
    pub fingerprint: String,
    pub irreversible_acknowledged: bool,
    pub typed_chat_title: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("select at least one message")]
    EmptySelection,
    #[error("none of the selected messages can currently be deleted for everyone")]
    NoEveryoneDeletable,
    #[error("selection contains {count} messages; the maximum is {max}")]
    SelectionTooLarge { count: usize, max: usize },
    #[error("{kind} identifier is invalid")]
    InvalidIdentifier { kind: &'static str },
    #[error("sender name is invalid")]
    InvalidSenderName,
    #[error("cleanup plan contains a message outside the target chat or beyond current authority")]
    InvalidOwnHistoryScope,
    #[error("inconsistent capability: {0}")]
    InconsistentCapability(&'static str),
    #[error("operation {0:?} is not allowed by Telegram's current capability flags")]
    OperationNotAllowed(PlanOperation),
    #[error("batch size must be between 1 and 100, got {0}")]
    InvalidBatchSize(usize),
    #[error("confirmation does not match the frozen deletion plan")]
    ConfirmationRejected,
}

fn plan_items(messages: Vec<MessageSnapshot>) -> Result<(Vec<PlanItem>, PlanSummary), DomainError> {
    if messages.is_empty() {
        return Err(DomainError::EmptySelection);
    }
    let (items, summary) = collect_plan_items(messages)?;
    if summary.delete_for_everyone == 0 {
        return Err(DomainError::NoEveryoneDeletable);
    }
    Ok((items, summary))
}

fn collect_plan_items(
    messages: Vec<MessageSnapshot>,
) -> Result<(Vec<PlanItem>, PlanSummary), DomainError> {
    if messages.len() > MAX_PLAN_MESSAGES {
        return Err(DomainError::SelectionTooLarge {
            count: messages.len(),
            max: MAX_PLAN_MESSAGES,
        });
    }

    let mut seen = BTreeSet::new();
    let mut items = Vec::with_capacity(messages.len());
    let mut summary = PlanSummary::default();
    for message in messages {
        validate_chat_identifier(message.chat_id)?;
        validate_message_identifier(message.message_id)?;
        if !seen.insert((message.chat_id, message.message_id)) {
            continue;
        }
        match message.deletion_reach {
            DeletionReach::Everyone => summary.delete_for_everyone += 1,
            DeletionReach::SelfOnly => summary.self_only += 1,
            DeletionReach::None => summary.cannot_delete += 1,
        }
        items.push(PlanItem {
            chat_id: message.chat_id,
            message_id: message.message_id,
            expected_reach: message.deletion_reach,
        });
    }
    summary.selected = items.len();
    Ok((items, summary))
}

fn validate_chat_identifier(value: i64) -> Result<(), DomainError> {
    // TDLib chat identifiers are signed int53 values; group and channel IDs are
    // commonly negative. Zero is the only universal sentinel we reject here.
    if value == 0 {
        return Err(DomainError::InvalidIdentifier { kind: "chat" });
    }
    Ok(())
}

fn validate_message_identifier(value: i64) -> Result<(), DomainError> {
    // Deletion plans contain only fully-sent server messages. TDLib may expose
    // temporary negative IDs while sending, but those must never reach a
    // destructive server-side plan.
    if value <= 0 {
        return Err(DomainError::InvalidIdentifier { kind: "message" });
    }
    Ok(())
}

fn fingerprint(
    operation: PlanOperation,
    target_chat_id: Option<i64>,
    target_sender_id: Option<i64>,
    items: &[PlanItem],
    title: Option<&str>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(format!("{operation:?}\n"));
    if let Some(chat_id) = target_chat_id {
        digest.update(chat_id.to_be_bytes());
    }
    if let Some(sender_id) = target_sender_id {
        digest.update(sender_id.to_be_bytes());
    }
    if let Some(title) = title {
        digest.update(title.as_bytes());
        digest.update(b"\n");
    }
    for item in items {
        digest.update(item.chat_id.to_be_bytes());
        digest.update(item.message_id.to_be_bytes());
        digest.update([item.expected_reach as u8]);
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(chat_id: i64, id: i64, reach: DeletionReach) -> MessageSnapshot {
        MessageSnapshot {
            chat_id,
            message_id: id,
            sender_id: 1,
            sender_name: "A".into(),
            sent_at: Utc::now(),
            is_outgoing: true,
            content_kind: ContentKind::Text,
            preview: "private content".into(),
            privacy_findings: Vec::new(),
            album_id: None,
            is_pinned: false,
            deletion_reach: reach,
        }
    }

    #[test]
    fn partitions_and_deduplicates_without_expanding_scope() {
        let plan = DeletionPlan::selected_messages(vec![
            message(1, 10, DeletionReach::Everyone),
            message(1, 10, DeletionReach::Everyone),
            message(1, 11, DeletionReach::SelfOnly),
            message(2, 20, DeletionReach::None),
        ])
        .unwrap();

        assert_eq!(plan.items.len(), 3);
        assert_eq!(plan.summary.selected, 3);
        assert_eq!(plan.summary.delete_for_everyone, 1);
        assert_eq!(plan.summary.self_only, 1);
        assert_eq!(plan.summary.cannot_delete, 1);
    }

    #[test]
    fn batches_are_chat_scoped_and_everyone_only() {
        let mut snapshots = Vec::new();
        for id in 1..=105 {
            snapshots.push(message(1, id, DeletionReach::Everyone));
        }
        snapshots.push(message(2, 1, DeletionReach::Everyone));
        snapshots.push(message(2, 2, DeletionReach::SelfOnly));

        let batches = DeletionPlan::selected_messages(snapshots)
            .unwrap()
            .everyone_batches(100)
            .unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].chat_id, 1);
        assert_eq!(batches[0].message_ids.len(), 100);
        assert_eq!(batches[1].message_ids.len(), 5);
        assert_eq!(batches[2].chat_id, 2);
        assert_eq!(batches[2].message_ids, vec![1]);
    }

    #[test]
    fn confirmation_is_bound_to_fingerprint_and_title() {
        let chat = ChatSummary {
            id: 1,
            title: "Design Team".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(20),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::Owner,
                can_delete_others: true,
                can_clear_for_everyone: true,
                can_remove_for_self: true,
                can_delete_group: true,
                can_delete_by_sender: true,
                can_leave_chat: false,
            },
            avatar_seed: 1,
        };
        let plan = DeletionPlan::chat_wide(PlanOperation::DeleteGroup, &chat).unwrap();
        let bad = ConfirmationProof {
            fingerprint: plan.fingerprint.clone(),
            irreversible_acknowledged: true,
            typed_chat_title: Some("Wrong".into()),
        };
        assert_eq!(
            plan.verify_confirmation(&bad),
            Err(DomainError::ConfirmationRejected)
        );

        let good = ConfirmationProof {
            typed_chat_title: Some(chat.title),
            ..bad
        };
        assert!(plan.verify_confirmation(&good).is_ok());
    }

    #[test]
    fn limited_admin_cannot_claim_group_deletion() {
        let capabilities = ChatCapabilities {
            role: ChatRole::AdminLimited,
            can_delete_others: false,
            can_clear_for_everyone: false,
            can_remove_for_self: false,
            can_delete_group: true,
            can_delete_by_sender: false,
            can_leave_chat: false,
        };
        assert!(capabilities.validate(ChatKind::Supergroup).is_err());
    }

    #[test]
    fn rejects_selection_with_no_everyone_capability() {
        let error = DeletionPlan::selected_messages(vec![message(1, 10, DeletionReach::SelfOnly)])
            .unwrap_err();
        assert_eq!(error, DomainError::NoEveryoneDeletable);
    }

    #[test]
    fn sender_plan_is_bound_to_chat_and_sender() {
        let chat = ChatSummary {
            id: -100,
            title: "Moderated".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(10),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::AdminWithDelete,
                can_delete_others: true,
                can_clear_for_everyone: true,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: true,
                can_leave_chat: true,
            },
            avatar_seed: 1,
        };
        let first = DeletionPlan::by_sender(&chat, 77, "Sender".into()).unwrap();
        let second = DeletionPlan::by_sender(&chat, 78, "Sender".into()).unwrap();
        assert_ne!(first.fingerprint, second.fingerprint);
        assert_eq!(first.target_sender_id, Some(77));
    }

    #[test]
    fn non_owner_group_member_can_plan_an_explicit_leave() {
        let chat = ChatSummary {
            id: -200,
            title: "Empty Group".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(22),
            conversation_state: ConversationState::Empty,
            capabilities: ChatCapabilities {
                role: ChatRole::Member,
                can_delete_others: false,
                can_clear_for_everyone: false,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: false,
                can_leave_chat: true,
            },
            avatar_seed: 2,
        };

        let plan = DeletionPlan::leave_chat(&chat, Vec::new()).unwrap();
        assert_eq!(plan.confirmation_tier, ConfirmationTier::Medium);
        assert_eq!(plan.summary.selected, 0);
        assert_eq!(plan.target_chat_id, Some(chat.id));
        assert!(
            plan.verify_confirmation(&ConfirmationProof {
                fingerprint: plan.fingerprint.clone(),
                irreversible_acknowledged: true,
                typed_chat_title: None,
            })
            .is_ok()
        );
    }

    #[test]
    fn leaving_freezes_revocable_outgoing_messages_before_membership_removal() {
        let chat = ChatSummary {
            id: -201,
            title: "Active Group".into(),
            kind: ChatKind::BasicGroup,
            archived: false,
            member_count: Some(3),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::Member,
                can_delete_others: false,
                can_clear_for_everyone: false,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: false,
                can_leave_chat: true,
            },
            avatar_seed: 3,
        };
        let mut revocable = message(chat.id, 20, DeletionReach::Everyone);
        revocable.is_outgoing = true;
        let mut local_only = message(chat.id, 21, DeletionReach::SelfOnly);
        local_only.is_outgoing = true;

        let plan = DeletionPlan::leave_chat(&chat, vec![revocable, local_only]).unwrap();
        assert_eq!(plan.confirmation_tier, ConfirmationTier::High);
        assert_eq!(plan.summary.selected, 2);
        assert_eq!(plan.summary.delete_for_everyone, 1);
        assert_eq!(plan.summary.self_only, 1);
        assert!(
            plan.verify_confirmation(&ConfirmationProof {
                fingerprint: plan.fingerprint.clone(),
                irreversible_acknowledged: true,
                typed_chat_title: Some(chat.title),
            })
            .is_ok()
        );
    }

    #[test]
    fn admin_leave_plan_freezes_every_deletable_message_not_only_outgoing_ones() {
        let chat = ChatSummary {
            id: -202,
            title: "Admin Group".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(5),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::AdminWithDelete,
                can_delete_others: true,
                can_clear_for_everyone: false,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: true,
                can_leave_chat: true,
            },
            avatar_seed: 4,
        };
        let mut mine = message(chat.id, 30, DeletionReach::Everyone);
        mine.is_outgoing = true;
        let mut theirs = message(chat.id, 31, DeletionReach::Everyone);
        theirs.is_outgoing = false;

        let plan = DeletionPlan::leave_chat(&chat, vec![mine, theirs]).unwrap();
        assert_eq!(format!("{:?}", plan.operation), "DeleteAllMessagesAndLeave");
        assert_eq!(plan.summary.selected, 2);
        assert_eq!(plan.summary.delete_for_everyone, 2);
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.confirmation_tier, ConfirmationTier::High);
    }

    #[test]
    fn whole_history_authority_makes_leave_a_high_impact_plan_without_enumeration() {
        let chat = ChatSummary {
            id: -203,
            title: "Clearable Group".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(5),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::AdminWithDelete,
                can_delete_others: true,
                can_clear_for_everyone: true,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: true,
                can_leave_chat: true,
            },
            avatar_seed: 5,
        };

        let plan = DeletionPlan::leave_chat(&chat, Vec::new()).unwrap();
        assert_eq!(format!("{:?}", plan.operation), "ClearHistoryAndLeave");
        assert_eq!(plan.confirmation_tier, ConfirmationTier::High);
    }

    #[test]
    fn direct_chat_can_be_removed_only_for_the_current_account() {
        let chat = ChatSummary {
            id: 304,
            title: "Deleted account".into(),
            kind: ChatKind::Direct,
            archived: false,
            member_count: None,
            conversation_state: ConversationState::Empty,
            capabilities: ChatCapabilities {
                role: ChatRole::Member,
                can_delete_others: false,
                can_clear_for_everyone: false,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: false,
                can_leave_chat: false,
            },
            avatar_seed: 4,
        };

        let plan = DeletionPlan::chat_wide(PlanOperation::RemoveChatForSelf, &chat).unwrap();
        assert_eq!(plan.confirmation_tier, ConfirmationTier::Medium);
        assert_eq!(plan.target_chat_id, Some(chat.id));
        assert_eq!(plan.summary, PlanSummary::default());
    }

    #[test]
    fn own_history_plan_is_group_bound_and_keeps_only_outgoing_messages() {
        let chat = ChatSummary {
            id: -300,
            title: "Community".into(),
            kind: ChatKind::Supergroup,
            archived: false,
            member_count: Some(30),
            conversation_state: ConversationState::Active,
            capabilities: ChatCapabilities {
                role: ChatRole::Member,
                can_delete_others: false,
                can_clear_for_everyone: false,
                can_remove_for_self: true,
                can_delete_group: false,
                can_delete_by_sender: false,
                can_leave_chat: true,
            },
            avatar_seed: 3,
        };
        let plan = DeletionPlan::own_messages(
            &chat,
            vec![
                message(chat.id, 1, DeletionReach::Everyone),
                message(chat.id, 2, DeletionReach::SelfOnly),
            ],
        )
        .unwrap();
        assert_eq!(plan.operation, PlanOperation::DeleteMyMessages);
        assert_eq!(plan.target_chat_id, Some(chat.id));
        assert_eq!(plan.summary.delete_for_everyone, 1);
        assert_eq!(plan.confirmation_tier, ConfirmationTier::High);

        let mut incoming = message(chat.id, 3, DeletionReach::Everyone);
        incoming.is_outgoing = false;
        assert_eq!(
            DeletionPlan::own_messages(&chat, vec![incoming]),
            Err(DomainError::InvalidOwnHistoryScope)
        );
    }
}
