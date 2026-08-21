use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cleaner_domain::{
    ChatCapabilities, ChatKind, ChatRole, ChatSummary, ContentKind, ConversationState,
    DeletionReach, MessageSnapshot, detect_sensitive_data,
};
use tokio::sync::RwLock;

use crate::{
    error::AppError,
    gateway::{GatewayInfo, TelegramGateway},
    model::{CatalogProgress, MessageDirection, SearchRequest},
};

#[derive(Clone)]
struct StoredMessage {
    snapshot: MessageSnapshot,
    deleted: bool,
}

struct DemoData {
    chats: Vec<ChatSummary>,
    messages: Vec<StoredMessage>,
}

pub struct DemoGateway {
    data: RwLock<DemoData>,
    reason: String,
    #[cfg(test)]
    chat_list_reads: AtomicUsize,
    #[cfg(test)]
    direct_chat_reads: AtomicUsize,
}

impl DemoGateway {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(seed_data()),
            reason:
                "No Telegram session is connected. Destructive actions affect demo fixtures only."
                    .into(),
            #[cfg(test)]
            chat_list_reads: AtomicUsize::new(0),
            #[cfg(test)]
            direct_chat_reads: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn chat_read_counts(&self) -> (usize, usize) {
        (
            self.chat_list_reads.load(Ordering::Acquire),
            self.direct_chat_reads.load(Ordering::Acquire),
        )
    }
}

#[async_trait]
impl TelegramGateway for DemoGateway {
    fn info(&self) -> GatewayInfo {
        GatewayInfo {
            mode: "demo",
            account_label: "Private demo account".into(),
            reason: Some(self.reason.clone()),
        }
    }

    fn auth(&self) -> crate::model::AuthSnapshot {
        crate::model::AuthSnapshot::ready()
    }

    fn catalog_progress(&self) -> CatalogProgress {
        let count = self
            .data
            .try_read()
            .map(|data| data.chats.len())
            .unwrap_or_default();
        CatalogProgress {
            phase: "ready",
            total: count,
            processed: count,
        }
    }

    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError> {
        #[cfg(test)]
        self.chat_list_reads.fetch_add(1, Ordering::AcqRel);
        let data = self.data.read().await;
        let mut chats = data.chats.clone();
        for chat in &mut chats {
            chat.conversation_state = conversation_state(chat.kind, chat.id, &data.messages);
        }
        Ok(chats)
    }

    async fn chat_by_id(&self, chat_id: i64) -> Result<Option<ChatSummary>, AppError> {
        #[cfg(test)]
        self.direct_chat_reads.fetch_add(1, Ordering::AcqRel);
        let data = self.data.read().await;
        Ok(data
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .cloned()
            .map(|mut chat| {
                chat.conversation_state = conversation_state(chat.kind, chat.id, &data.messages);
                chat
            }))
    }

    async fn search(&self, request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError> {
        let data = self.data.read().await;
        let chat_kinds: HashMap<i64, ChatKind> =
            data.chats.iter().map(|chat| (chat.id, chat.kind)).collect();
        let query_tokens: Vec<String> = request
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let chat_ids: HashSet<i64> = request.chat_ids.iter().copied().collect();

        let mut results: Vec<_> = data
            .messages
            .iter()
            .filter(|stored| !stored.deleted)
            .map(|stored| &stored.snapshot)
            .filter(|message| chat_kinds.contains_key(&message.chat_id))
            .filter(|message| chat_ids.is_empty() || chat_ids.contains(&message.chat_id))
            .filter(|message| {
                request.chat_kinds.is_empty()
                    || chat_kinds
                        .get(&message.chat_id)
                        .is_some_and(|kind| request.chat_kinds.contains(kind))
            })
            .filter(|message| {
                request.content_kinds.is_empty()
                    || request.content_kinds.contains(&message.content_kind)
            })
            .filter(|message| match request.direction {
                MessageDirection::Any => true,
                MessageDirection::Mine => message.is_outgoing,
                MessageDirection::Others => !message.is_outgoing,
            })
            .filter(|message| !request.exclude_pinned || !message.is_pinned)
            .filter(|message| {
                request
                    .min_date
                    .is_none_or(|minimum| message.sent_at >= minimum)
                    && request
                        .max_date
                        .is_none_or(|maximum| message.sent_at <= maximum)
            })
            .filter(|message| {
                if query_tokens.is_empty() {
                    return true;
                }
                let searchable =
                    format!("{} {}", message.preview, message.sender_name).to_lowercase();
                query_tokens.iter().all(|token| searchable.contains(token))
            })
            .cloned()
            .collect();
        if request.privacy_scan {
            for message in &mut results {
                message.privacy_findings =
                    detect_sensitive_data(&message.preview, message.content_kind);
            }
            results.retain(|message| !message.privacy_findings.is_empty());
        }
        results.sort_by_key(|message| std::cmp::Reverse(message.sent_at));
        results.truncate(request.limit);
        Ok(results)
    }

    async fn own_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        let data = self.data.read().await;
        Ok(data
            .messages
            .iter()
            .filter(|stored| {
                !stored.deleted && stored.snapshot.chat_id == chat_id && stored.snapshot.is_outgoing
            })
            .map(|stored| stored.snapshot.clone())
            .collect())
    }

    async fn chat_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        let data = self.data.read().await;
        Ok(data
            .messages
            .iter()
            .filter(|stored| !stored.deleted && stored.snapshot.chat_id == chat_id)
            .map(|stored| stored.snapshot.clone())
            .collect())
    }

    async fn messages_by_ids(&self, ids: &[(i64, i64)]) -> Result<Vec<MessageSnapshot>, AppError> {
        let wanted: HashSet<(i64, i64)> = ids.iter().copied().collect();
        let data = self.data.read().await;
        Ok(data
            .messages
            .iter()
            .filter(|stored| {
                !stored.deleted
                    && wanted.contains(&(stored.snapshot.chat_id, stored.snapshot.message_id))
            })
            .map(|stored| stored.snapshot.clone())
            .collect())
    }

    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError> {
        let data = self.data.read().await;
        Ok(data
            .messages
            .iter()
            .find(|stored| {
                !stored.deleted
                    && stored.snapshot.chat_id == chat_id
                    && stored.snapshot.message_id == message_id
            })
            .map(|stored| stored.snapshot.deletion_reach))
    }

    async fn delete_messages_for_everyone(
        &self,
        chat_id: i64,
        message_ids: &[i64],
    ) -> Result<(), AppError> {
        if message_ids.is_empty() || message_ids.len() > 100 {
            return Err(AppError::Gateway("invalid deletion batch".into()));
        }
        let wanted: HashSet<i64> = message_ids.iter().copied().collect();
        let mut data = self.data.write().await;
        for message_id in &wanted {
            let Some(stored) = data.messages.iter().find(|stored| {
                !stored.deleted
                    && stored.snapshot.chat_id == chat_id
                    && stored.snapshot.message_id == *message_id
            }) else {
                return Err(AppError::Gateway("MESSAGE_NOT_FOUND".into()));
            };
            if stored.snapshot.deletion_reach != DeletionReach::Everyone {
                return Err(AppError::Gateway("MESSAGE_DELETE_FORBIDDEN".into()));
            }
        }
        for stored in &mut data.messages {
            if stored.snapshot.chat_id == chat_id && wanted.contains(&stored.snapshot.message_id) {
                stored.deleted = true;
            }
        }
        Ok(())
    }

    async fn clear_history_for_everyone(&self, chat_id: i64) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !chat.capabilities.can_clear_for_everyone {
            return Err(AppError::Gateway("CHAT_ADMIN_REQUIRED".into()));
        }
        for stored in &mut data.messages {
            if stored.snapshot.chat_id == chat_id {
                stored.deleted = true;
            }
        }
        data.chats.retain(|chat| chat.id != chat_id);
        Ok(())
    }

    async fn clear_history_for_everyone_keep_chat(&self, chat_id: i64) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !chat.capabilities.can_clear_for_everyone {
            return Err(AppError::Gateway("CHAT_ADMIN_REQUIRED".into()));
        }
        for stored in &mut data.messages {
            if stored.snapshot.chat_id == chat_id {
                stored.deleted = true;
            }
        }
        if let Some(chat) = data.chats.iter_mut().find(|chat| chat.id == chat_id) {
            // TDLib can stop advertising a whole-history deletion capability
            // after that history has already been cleared. Leaving must depend
            // on membership, not on this now-consumed capability.
            chat.capabilities.can_clear_for_everyone = false;
        }
        Ok(())
    }

    async fn remove_chat_for_self(&self, chat_id: i64) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let index = data
            .chats
            .iter()
            .position(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !data.chats[index].capabilities.can_remove_for_self {
            return Err(AppError::Gateway("CHAT_DELETE_FOR_SELF_FORBIDDEN".into()));
        }
        data.chats.remove(index);
        data.messages
            .retain(|stored| stored.snapshot.chat_id != chat_id);
        Ok(())
    }

    async fn delete_group(&self, chat_id: i64) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let index = data
            .chats
            .iter()
            .position(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !data.chats[index].capabilities.can_delete_group {
            return Err(AppError::Gateway("CHAT_ADMIN_REQUIRED".into()));
        }
        data.chats.remove(index);
        data.messages
            .retain(|stored| stored.snapshot.chat_id != chat_id);
        Ok(())
    }

    async fn leave_chat(&self, chat_id: i64) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let index = data
            .chats
            .iter()
            .position(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !data.chats[index].capabilities.can_leave_chat {
            return Err(AppError::Gateway("CHAT_MEMBER_REQUIRED".into()));
        }
        data.chats.remove(index);
        Ok(())
    }

    async fn delete_messages_by_sender(
        &self,
        chat_id: i64,
        sender_id: i64,
    ) -> Result<(), AppError> {
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !chat.capabilities.can_delete_by_sender {
            return Err(AppError::Gateway("CHAT_ADMIN_REQUIRED".into()));
        }
        for stored in &mut data.messages {
            if stored.snapshot.chat_id == chat_id && stored.snapshot.sender_id == sender_id {
                stored.deleted = true;
            }
        }
        Ok(())
    }

    async fn request_qr_auth(&self) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn submit_phone(&self, _phone: &str) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn submit_email_address(&self, _email: &str) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn submit_email_code(&self, _code: &str) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn submit_code(&self, _code: &str) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn submit_password(&self, _password: &str) -> Result<(), AppError> {
        Err(demo_auth_error())
    }
    async fn close(&self) -> Result<(), AppError> {
        Ok(())
    }
}

fn demo_auth_error() -> AppError {
    AppError::InvalidRequest("authentication is unavailable in safe demo mode".into())
}

fn seed_data() -> DemoData {
    let chats = vec![
        chat(
            101,
            "Maya Chen",
            ChatKind::Direct,
            false,
            None,
            ChatRole::Member,
            false,
            true,
            false,
            false,
            2,
        ),
        chat(
            -1001,
            "Design Team",
            ChatKind::Supergroup,
            false,
            Some(24),
            ChatRole::Owner,
            true,
            true,
            true,
            true,
            5,
        ),
        chat(
            -1002,
            "Neighborhood Exchange",
            ChatKind::Supergroup,
            false,
            Some(418),
            ChatRole::AdminWithDelete,
            true,
            true,
            false,
            true,
            8,
        ),
        chat(
            -1003,
            "Volunteer Archive",
            ChatKind::Supergroup,
            true,
            Some(82),
            ChatRole::AdminWithDelete,
            true,
            false,
            false,
            true,
            11,
        ),
        chat(
            -1004,
            "Open Source News",
            ChatKind::Channel,
            false,
            Some(3_204),
            ChatRole::Member,
            false,
            false,
            false,
            false,
            14,
        ),
        chat(
            202,
            "Old devices",
            ChatKind::Secret,
            true,
            None,
            ChatRole::Member,
            false,
            true,
            false,
            false,
            17,
        ),
        chat(
            303,
            "Prize Support",
            ChatKind::Direct,
            false,
            None,
            ChatRole::Member,
            false,
            true,
            false,
            false,
            20,
        ),
        chat(
            304,
            "Empty invite",
            ChatKind::Direct,
            false,
            None,
            ChatRole::Member,
            false,
            false,
            false,
            false,
            23,
        ),
    ];

    let mut messages = Vec::new();
    let fixtures = [
        (
            101,
            1,
            501,
            "Maya",
            false,
            ContentKind::Text,
            "The temporary address was 17 Juniper Lane.",
            DeletionReach::Everyone,
            false,
        ),
        (
            101,
            2,
            42,
            "You",
            true,
            ContentKind::Photo,
            "Passport scan for the apartment application",
            DeletionReach::Everyone,
            false,
        ),
        (
            101,
            3,
            501,
            "Maya",
            false,
            ContentKind::Text,
            "I deleted the shared folder already.",
            DeletionReach::Everyone,
            false,
        ),
        (
            101,
            4,
            42,
            "You",
            true,
            ContentKind::Voice,
            "Voice message · 0:18",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1001,
            11,
            42,
            "You",
            true,
            ContentKind::Text,
            "Project Cedar launch credentials moved to the vault.",
            DeletionReach::Everyone,
            true,
        ),
        (
            -1001,
            12,
            712,
            "Nora",
            false,
            ContentKind::File,
            "cedar_research_notes.pdf · 4.8 MB",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1001,
            13,
            713,
            "Owen",
            false,
            ContentKind::Photo,
            "Whiteboard with customer email list",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1001,
            14,
            42,
            "You",
            true,
            ContentKind::Text,
            "My old phone number ends in 0441.",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1001,
            15,
            714,
            "Priya",
            false,
            ContentKind::Poll,
            "Where should we hold the offsite?",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1002,
            21,
            818,
            "Unknown",
            false,
            ContentKind::Text,
            "Limited offer — contact me directly",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1002,
            22,
            818,
            "Unknown",
            false,
            ContentKind::Photo,
            "Advertisement image",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1002,
            23,
            42,
            "You",
            true,
            ContentKind::Location,
            "Old pickup point",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1002,
            24,
            819,
            "Jo",
            false,
            ContentKind::Contact,
            "Contact card · Alex R.",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1003,
            31,
            42,
            "You",
            true,
            ContentKind::Text,
            "Here is my personal email for the volunteer roster.",
            DeletionReach::Everyone,
            false,
        ),
        (
            -1003,
            32,
            920,
            "Sam",
            false,
            ContentKind::File,
            "volunteer_roster_2022.xlsx",
            DeletionReach::None,
            true,
        ),
        (
            -1003,
            33,
            921,
            "Lee",
            false,
            ContentKind::Text,
            "The archive should remain read-only.",
            DeletionReach::None,
            false,
        ),
        (
            -1004,
            41,
            1004,
            "Open Source News",
            false,
            ContentKind::Text,
            "Release notes for version 8.4",
            DeletionReach::None,
            false,
        ),
        (
            -1004,
            42,
            1004,
            "Open Source News",
            false,
            ContentKind::Video,
            "Conference keynote · 24:10",
            DeletionReach::None,
            false,
        ),
        (
            202,
            51,
            42,
            "You",
            true,
            ContentKind::Text,
            "Recovery phrase moved offline; delete this reminder.",
            DeletionReach::Everyone,
            false,
        ),
        (
            303,
            61,
            1303,
            "Prize Support",
            false,
            ContentKind::Text,
            "You won a prize — reply with your account details",
            DeletionReach::Everyone,
            false,
        ),
        (
            202,
            52,
            1202,
            "Old devices",
            false,
            ContentKind::Text,
            "This secret chat only exists on this device.",
            DeletionReach::Everyone,
            false,
        ),
        (
            101,
            5,
            42,
            "You",
            true,
            ContentKind::Text,
            "Backup contact person@example.com · wallet 0x52908400098527886E0F7030069857D2E4169EE7",
            DeletionReach::Everyone,
            false,
        ),
    ];

    for (index, fixture) in fixtures.into_iter().enumerate() {
        let (chat_id, message_id, sender_id, sender, outgoing, kind, preview, reach, pinned) =
            fixture;
        messages.push(StoredMessage {
            snapshot: MessageSnapshot {
                chat_id,
                message_id,
                sender_id,
                sender_name: sender.into(),
                sent_at: Utc
                    .with_ymd_and_hms(2026, 8, 15 - (index as u32 / 5), 18, index as u32, 0)
                    .single()
                    .expect("valid demo timestamp"),
                is_outgoing: outgoing,
                content_kind: kind,
                preview: preview.into(),
                privacy_findings: Vec::new(),
                album_id: matches!(message_id, 12 | 13).then_some(7001),
                is_pinned: pinned,
                deletion_reach: reach,
            },
            deleted: false,
        });
    }

    DemoData { chats, messages }
}

#[allow(clippy::too_many_arguments)]
fn chat(
    id: i64,
    title: &str,
    kind: ChatKind,
    archived: bool,
    member_count: Option<u32>,
    role: ChatRole,
    can_delete_others: bool,
    can_clear_for_everyone: bool,
    can_delete_group: bool,
    can_delete_by_sender: bool,
    avatar_seed: u8,
) -> ChatSummary {
    ChatSummary {
        id,
        title: title.into(),
        kind,
        archived,
        member_count,
        conversation_state: ConversationState::Unknown,
        capabilities: ChatCapabilities {
            role,
            can_delete_others,
            can_clear_for_everyone,
            can_remove_for_self: matches!(kind, ChatKind::Direct | ChatKind::Secret),
            can_delete_group,
            can_delete_by_sender,
            can_leave_chat: matches!(
                kind,
                ChatKind::BasicGroup | ChatKind::Supergroup | ChatKind::Channel
            ) && role != ChatRole::Owner,
        },
        avatar_seed,
    }
}

fn conversation_state(
    kind: ChatKind,
    chat_id: i64,
    messages: &[StoredMessage],
) -> ConversationState {
    if kind == ChatKind::Channel {
        return ConversationState::Unknown;
    }
    let visible: Vec<_> = messages
        .iter()
        .filter(|stored| !stored.deleted && stored.snapshot.chat_id == chat_id)
        .collect();
    let Some(latest) = visible.iter().max_by_key(|stored| stored.snapshot.sent_at) else {
        return ConversationState::Empty;
    };
    if latest.snapshot.is_outgoing {
        ConversationState::Active
    } else if visible.iter().any(|stored| stored.snapshot.is_outgoing) {
        ConversationState::AwaitingReply
    } else {
        ConversationState::NeverReplied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_empty_and_unanswered_cleanup_candidates() {
        tauri::async_runtime::block_on(async {
            let gateway = DemoGateway::new();
            let chats = gateway.chats().await.unwrap();
            assert_eq!(
                chats
                    .iter()
                    .find(|chat| chat.id == 303)
                    .unwrap()
                    .conversation_state,
                ConversationState::NeverReplied
            );
            assert_eq!(
                chats
                    .iter()
                    .find(|chat| chat.id == 304)
                    .unwrap()
                    .conversation_state,
                ConversationState::Empty
            );

            gateway.clear_history_for_everyone(303).await.unwrap();
            assert!(
                gateway
                    .chats()
                    .await
                    .unwrap()
                    .iter()
                    .all(|chat| chat.id != 303)
            );

            gateway.remove_chat_for_self(304).await.unwrap();
            assert!(
                gateway
                    .chats()
                    .await
                    .unwrap()
                    .iter()
                    .all(|chat| chat.id != 304)
            );
        });
    }
}
