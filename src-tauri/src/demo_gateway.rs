use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cleaner_domain::{
    ChatCapabilities, ChatKind, ChatRole, ChatSummary, ContentKind, ConversationState,
    DeletionReach, MessageSnapshot, detect_sensitive_data,
};
use tokio::sync::RwLock;
#[cfg(test)]
use tokio::sync::{Mutex, Notify};

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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestFailurePoint {
    DeleteMessagesForEveryone,
    ClearHistoryForEveryone,
}

pub struct DemoGateway {
    data: RwLock<DemoData>,
    reason: String,
    verified_identity: Option<crate::providers::telegram::identity::VerifiedTelegramIdentity>,
    #[cfg(test)]
    chat_list_reads: AtomicUsize,
    #[cfg(test)]
    direct_chat_reads: AtomicUsize,
    #[cfg(test)]
    current_reach_delay_ms: AtomicU64,
    #[cfg(test)]
    current_reach_started: AtomicBool,
    #[cfg(test)]
    operation_log: Mutex<Vec<String>>,
    #[cfg(test)]
    delete_batch_sizes: Mutex<Vec<usize>>,
    #[cfg(test)]
    delete_calls: Mutex<Vec<(i64, Vec<i64>)>>,
    #[cfg(test)]
    current_reach_calls: Mutex<Vec<(i64, i64)>>,
    #[cfg(test)]
    chat_by_id_calls: Mutex<Vec<i64>>,
    #[cfg(test)]
    rate_limit_injections: Mutex<Vec<TestFailurePoint>>,
    #[cfg(test)]
    injected_failures_seen: AtomicUsize,
    #[cfg(test)]
    injected_failure_notify: Notify,
}

impl DemoGateway {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(seed_data()),
            reason:
                "No Telegram session is connected. Destructive actions affect demo fixtures only."
                    .into(),
            verified_identity: None,
            #[cfg(test)]
            chat_list_reads: AtomicUsize::new(0),
            #[cfg(test)]
            direct_chat_reads: AtomicUsize::new(0),
            #[cfg(test)]
            current_reach_delay_ms: AtomicU64::new(0),
            #[cfg(test)]
            current_reach_started: AtomicBool::new(false),
            #[cfg(test)]
            operation_log: Mutex::new(Vec::new()),
            #[cfg(test)]
            delete_batch_sizes: Mutex::new(Vec::new()),
            #[cfg(test)]
            delete_calls: Mutex::new(Vec::new()),
            #[cfg(test)]
            current_reach_calls: Mutex::new(Vec::new()),
            #[cfg(test)]
            chat_by_id_calls: Mutex::new(Vec::new()),
            #[cfg(test)]
            rate_limit_injections: Mutex::new(Vec::new()),
            #[cfg(test)]
            injected_failures_seen: AtomicUsize::new(0),
            #[cfg(test)]
            injected_failure_notify: Notify::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_verified_identity(
        identity: crate::providers::telegram::identity::VerifiedTelegramIdentity,
    ) -> Self {
        let mut gateway = Self::new();
        gateway.verified_identity = Some(identity);
        gateway
    }

    #[cfg(test)]
    pub(crate) fn chat_read_counts(&self) -> (usize, usize) {
        (
            self.chat_list_reads.load(Ordering::Acquire),
            self.direct_chat_reads.load(Ordering::Acquire),
        )
    }

    #[cfg(test)]
    pub(crate) fn delay_current_reach(&self, milliseconds: u64) {
        self.current_reach_started.store(false, Ordering::Release);
        self.current_reach_delay_ms
            .store(milliseconds, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn current_reach_started(&self) -> bool {
        self.current_reach_started.load(Ordering::Acquire)
    }

    #[cfg(test)]
    async fn record(&self, operation: String) {
        self.operation_log.lock().await.push(operation);
    }

    #[cfg(test)]
    pub(crate) async fn operation_log(&self) -> Vec<String> {
        self.operation_log.lock().await.clone()
    }

    #[cfg(test)]
    pub(crate) async fn clear_operation_log(&self) {
        self.operation_log.lock().await.clear();
    }

    #[cfg(test)]
    pub(crate) async fn clear_test_traces(&self) {
        self.operation_log.lock().await.clear();
        self.delete_batch_sizes.lock().await.clear();
        self.delete_calls.lock().await.clear();
        self.current_reach_calls.lock().await.clear();
        self.chat_by_id_calls.lock().await.clear();
        self.rate_limit_injections.lock().await.clear();
        self.injected_failures_seen.store(0, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) async fn delete_batch_sizes(&self) -> Vec<usize> {
        self.delete_batch_sizes.lock().await.clone()
    }

    #[cfg(test)]
    pub(crate) async fn delete_calls(&self) -> Vec<(i64, Vec<i64>)> {
        self.delete_calls.lock().await.clone()
    }

    #[cfg(test)]
    pub(crate) async fn current_reach_calls(&self) -> Vec<(i64, i64)> {
        self.current_reach_calls.lock().await.clone()
    }

    #[cfg(test)]
    pub(crate) async fn chat_by_id_calls(&self) -> Vec<i64> {
        self.chat_by_id_calls.lock().await.clone()
    }

    #[cfg(test)]
    pub(crate) async fn inject_rate_limit_once(&self, point: TestFailurePoint) {
        self.rate_limit_injections.lock().await.push(point);
    }

    #[cfg(test)]
    async fn take_rate_limit(&self, point: TestFailurePoint) -> bool {
        let mut injections = self.rate_limit_injections.lock().await;
        let Some(index) = injections.iter().position(|candidate| *candidate == point) else {
            return false;
        };
        injections.remove(index);
        drop(injections);
        self.injected_failures_seen.fetch_add(1, Ordering::AcqRel);
        self.injected_failure_notify.notify_one();
        true
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_injected_failure(&self, expected_count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if self.injected_failures_seen.load(Ordering::Acquire) >= expected_count {
                    return;
                }
                self.injected_failure_notify.notified().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("expected {expected_count} injected synthetic gateway failures")
        });
    }

    #[cfg(test)]
    pub(crate) async fn set_message_reach(
        &self,
        chat_id: i64,
        message_id: i64,
        reach: DeletionReach,
    ) {
        let mut data = self.data.write().await;
        let message = data
            .messages
            .iter_mut()
            .find(|stored| {
                stored.snapshot.chat_id == chat_id && stored.snapshot.message_id == message_id
            })
            .expect("synthetic message exists");
        message.snapshot.deletion_reach = reach;
    }

    #[cfg(test)]
    pub(crate) async fn set_chat_clear_authority(&self, chat_id: i64, allowed: bool) {
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter_mut()
            .find(|chat| chat.id == chat_id)
            .expect("synthetic chat exists");
        chat.capabilities.can_clear_for_everyone = allowed;
    }

    pub(crate) async fn set_chat_membership(&self, chat_id: i64, role: ChatRole, can_leave: bool) {
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter_mut()
            .find(|chat| chat.id == chat_id)
            .unwrap();
        chat.capabilities.role = role;
        chat.capabilities.can_leave_chat = can_leave;
    }

    #[cfg(test)]
    pub(crate) async fn append_messages(&self, chat_id: i64, first_message_id: i64, count: usize) {
        let mut data = self.data.write().await;
        assert!(data.chats.iter().any(|chat| chat.id == chat_id));
        let message_ids = (0..count)
            .map(|offset| {
                first_message_id
                    .checked_add(i64::try_from(offset).expect("synthetic message count fits i64"))
                    .expect("synthetic message ID does not overflow")
            })
            .collect::<Vec<_>>();
        assert!(message_ids.iter().all(|message_id| {
            *message_id > 0
                && data.messages.iter().all(|stored| {
                    stored.snapshot.chat_id != chat_id || stored.snapshot.message_id != *message_id
                })
        }));
        for (offset, message_id) in message_ids.into_iter().enumerate() {
            let offset = i64::try_from(offset).expect("synthetic message count fits i64");
            data.messages.push(StoredMessage {
                snapshot: MessageSnapshot {
                    chat_id,
                    message_id,
                    sender_id: 42,
                    sender_name: "You".into(),
                    sent_at: Utc
                        .timestamp_opt(1_700_000_000 + offset, 0)
                        .single()
                        .expect("valid synthetic timestamp"),
                    is_outgoing: true,
                    content_kind: ContentKind::Text,
                    preview: "Synthetic batch message".into(),
                    privacy_findings: Vec::new(),
                    album_id: None,
                    is_pinned: false,
                    deletion_reach: DeletionReach::Everyone,
                },
                deleted: false,
            });
        }
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

    fn verified_identity(
        &self,
    ) -> Option<crate::providers::telegram::identity::VerifiedTelegramIdentity> {
        self.verified_identity.clone()
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
        {
            self.direct_chat_reads.fetch_add(1, Ordering::AcqRel);
            self.chat_by_id_calls.lock().await.push(chat_id);
        }
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

    async fn sender_name(&self, sender_id: i64) -> Result<String, AppError> {
        let data = self.data.read().await;
        data.messages
            .iter()
            .find(|stored| stored.snapshot.sender_id == sender_id)
            .map(|stored| stored.snapshot.sender_name.clone())
            .ok_or(AppError::NotFound)
    }

    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError> {
        #[cfg(test)]
        {
            self.current_reach_calls
                .lock()
                .await
                .push((chat_id, message_id));
            self.current_reach_started.store(true, Ordering::Release);
            let delay = self.current_reach_delay_ms.load(Ordering::Acquire);
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
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
        #[cfg(test)]
        {
            let mut sorted_ids = message_ids.to_vec();
            sorted_ids.sort_unstable();
            let ids = sorted_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            self.record(format!("delete_messages_for_everyone:{chat_id}:{ids}"))
                .await;
            self.delete_batch_sizes.lock().await.push(message_ids.len());
            self.delete_calls
                .lock()
                .await
                .push((chat_id, message_ids.to_vec()));
            if self
                .take_rate_limit(TestFailurePoint::DeleteMessagesForEveryone)
                .await
            {
                return Err(AppError::Gateway("FLOOD_WAIT_1".into()));
            }
        }
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
        #[cfg(test)]
        {
            self.record(format!("clear_history_for_everyone:{chat_id}"))
                .await;
            if self
                .take_rate_limit(TestFailurePoint::ClearHistoryForEveryone)
                .await
            {
                return Err(AppError::Gateway("FLOOD_WAIT_1".into()));
            }
        }
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
        #[cfg(test)]
        self.record(format!("clear_history_for_everyone_keep_chat:{chat_id}"))
            .await;
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
        #[cfg(test)]
        self.record(format!("remove_chat_for_self:{chat_id}")).await;
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
        Ok(())
    }

    async fn delete_group(&self, chat_id: i64) -> Result<(), AppError> {
        #[cfg(test)]
        self.record(format!("delete_group:{chat_id}")).await;
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
        #[cfg(test)]
        self.record(format!("leave_chat:{chat_id}")).await;
        let mut data = self.data.write().await;
        let chat = data
            .chats
            .iter_mut()
            .find(|chat| chat.id == chat_id)
            .ok_or(AppError::NotFound)?;
        if !chat.capabilities.can_leave_chat {
            return Err(AppError::Gateway("CHAT_MEMBER_REQUIRED".into()));
        }
        chat.capabilities.can_leave_chat = false;
        chat.capabilities.can_remove_for_self = true;
        Ok(())
    }

    async fn delete_messages_by_sender(
        &self,
        chat_id: i64,
        sender_id: i64,
    ) -> Result<(), AppError> {
        #[cfg(test)]
        self.record(format!("delete_messages_by_sender:{chat_id}:{sender_id}"))
            .await;
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

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            chat_ids: Vec::new(),
            chat_kinds: Vec::new(),
            content_kinds: Vec::new(),
            direction: MessageDirection::Any,
            min_date: None,
            max_date: None,
            exclude_pinned: false,
            privacy_scan: false,
            limit: 500,
        }
    }

    #[test]
    fn telegram_search_contract_covers_normalized_filters() {
        tauri::async_runtime::block_on(async {
            let gateway = DemoGateway::new();

            let query_cases: [(&str, &[(i64, i64)]); 4] = [
                ("passport apartment", &[(101, 2)]),
                ("cedar vault", &[(-1001, 11)]),
                ("priya", &[(-1001, 15)]),
                ("synthetic-no-match", &[]),
            ];
            for (query, expected) in query_cases {
                let actual = gateway
                    .search(&request(query))
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|message| (message.chat_id, message.message_id))
                    .collect::<Vec<_>>();
                assert_eq!(actual.as_slice(), expected, "query case {query:?}");
            }

            let mut scoped = request("");
            scoped.chat_ids = vec![-1001];
            let scoped_results = gateway.search(&scoped).await.unwrap();
            assert!(!scoped_results.is_empty());
            assert!(scoped_results.iter().all(|m| m.chat_id == -1001));

            let mut outgoing = request("");
            outgoing.direction = MessageDirection::Mine;
            let outgoing_results = gateway.search(&outgoing).await.unwrap();
            assert!(!outgoing_results.is_empty());
            assert!(outgoing_results.iter().all(|m| m.is_outgoing));

            let mut files = request("");
            files.content_kinds = vec![ContentKind::File];
            let file_results = gateway.search(&files).await.unwrap();
            assert!(!file_results.is_empty());
            assert!(
                file_results
                    .iter()
                    .all(|m| m.content_kind == ContentKind::File)
            );

            let mut groups = request("");
            groups.chat_kinds = vec![ChatKind::Supergroup];
            let group_ids = [-1001, -1002, -1003];
            let group_results = gateway.search(&groups).await.unwrap();
            assert!(!group_results.is_empty());
            assert!(group_results.iter().all(|m| group_ids.contains(&m.chat_id)));

            let mut unpinned = request("");
            unpinned.exclude_pinned = true;
            let unpinned_results = gateway.search(&unpinned).await.unwrap();
            assert!(!unpinned_results.is_empty());
            assert!(unpinned_results.iter().all(|m| !m.is_pinned));

            let boundary = Utc
                .with_ymd_and_hms(2026, 8, 14, 18, 5, 0)
                .single()
                .unwrap();
            let mut since_boundary = request("");
            since_boundary.min_date = Some(boundary);
            let since_results = gateway.search(&since_boundary).await.unwrap();
            assert!(since_results.iter().all(|m| m.sent_at >= boundary));
            assert!(
                since_results
                    .iter()
                    .any(|m| { (m.chat_id, m.message_id, m.sent_at) == (-1001, 12, boundary) })
            );

            let mut through_boundary = request("");
            through_boundary.max_date = Some(boundary);
            let through_results = gateway.search(&through_boundary).await.unwrap();
            assert!(through_results.iter().all(|m| m.sent_at <= boundary));
            assert!(
                through_results
                    .iter()
                    .any(|m| { (m.chat_id, m.message_id, m.sent_at) == (-1001, 12, boundary) })
            );

            let mut limited = request("");
            limited.limit = 1;
            let limited_results = gateway.search(&limited).await.unwrap();
            assert_eq!(limited_results.len(), 1);
            assert_eq!(
                (limited_results[0].chat_id, limited_results[0].message_id),
                (-1001, 11)
            );

            let mut privacy_scan = request("");
            privacy_scan.privacy_scan = true;
            let privacy_results = gateway.search(&privacy_scan).await.unwrap();
            assert!(!privacy_results.is_empty());
            assert!(
                privacy_results
                    .iter()
                    .all(|message| !message.privacy_findings.is_empty())
            );
            assert!(
                privacy_results
                    .iter()
                    .find(|message| (message.chat_id, message.message_id) == (101, 5))
                    .unwrap()
                    .privacy_findings
                    .contains(&cleaner_domain::SensitiveDataKind::CryptoWallet)
            );
            assert!(
                privacy_results
                    .iter()
                    .find(|message| (message.chat_id, message.message_id) == (101, 2))
                    .unwrap()
                    .privacy_findings
                    .contains(&cleaner_domain::SensitiveDataKind::IdentityDocument)
            );
        });
    }

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
