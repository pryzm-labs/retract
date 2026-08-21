use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, RwLock as SyncRwLock,
        atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use cleaner_domain::{
    ChatCapabilities, ChatKind, ChatRole, ChatSummary, ContentKind, ConversationState,
    DeletionReach, MessageSnapshot, detect_sensitive_data,
};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

use crate::{
    error::AppError,
    gateway::{GatewayInfo, TelegramGateway},
    model::{AuthSnapshot, AuthStage, CatalogProgress, MessageDirection, SearchRequest},
    tdjson::TdJsonClient,
};

pub const SUPPORTED_TDLIB_VERSION: &str = "1.8.64";
const CHAT_SUMMARY_CONCURRENCY: usize = 12;
const CHAT_CLEANUP_MESSAGE_LIMIT: usize = 100_001;
const MESSAGE_MAPPING_CONCURRENCY: usize = 12;
const CATALOG_IDLE: u8 = 0;
const CATALOG_DISCOVERING: u8 = 1;
const CATALOG_LOADING: u8 = 2;
const CATALOG_READY: u8 = 3;

pub struct LiveGatewayConfig {
    pub library_path: PathBuf,
    pub api_id: i32,
    pub api_hash: Zeroizing<String>,
    pub data_directory: PathBuf,
    pub database_key: Zeroizing<[u8; 32]>,
    pub use_test_dc: bool,
}

impl LiveGatewayConfig {
    pub fn new(
        library_path: PathBuf,
        api_id: i32,
        api_hash: Zeroizing<String>,
        use_test_dc: bool,
        data_directory: PathBuf,
        database_key: [u8; 32],
    ) -> Self {
        Self {
            library_path,
            api_id,
            api_hash,
            data_directory,
            database_key: Zeroizing::new(database_key),
            use_test_dc,
        }
    }
}

pub struct LiveGateway {
    client: TdJsonClient,
    config: LiveGatewayConfig,
    auth: SyncRwLock<AuthSnapshot>,
    account_label: SyncRwLock<String>,
    own_user_id: AtomicI64,
    sender_names: RwLock<HashMap<(bool, i64), String>>,
    chat_kinds: RwLock<HashMap<i64, ChatKind>>,
    catalog_loaded: AtomicBool,
    search_generation: AtomicU64,
    version_compatible: AtomicBool,
    catalog_load_lock: Mutex<()>,
    chat_summary_cache: RwLock<Option<Vec<ChatSummary>>>,
    chat_summary_load_lock: Mutex<()>,
    dirty_chat_summaries: Mutex<HashSet<i64>>,
    group_chat_ids: RwLock<HashMap<(bool, i64), i64>>,
    catalog_phase: AtomicU8,
    catalog_total: AtomicUsize,
    catalog_processed: AtomicUsize,
}

impl LiveGateway {
    pub fn connect(config: LiveGatewayConfig) -> Result<Arc<Self>, AppError> {
        std::fs::create_dir_all(config.data_directory.join("database"))?;
        std::fs::create_dir_all(config.data_directory.join("files"))?;
        let client = TdJsonClient::load(&config.library_path)?;
        let gateway = Arc::new(Self {
            client,
            config,
            auth: SyncRwLock::new(AuthSnapshot {
                stage: AuthStage::Initializing,
                hint: Some("Starting the local Telegram engine…".into()),
                qr_link: None,
            }),
            account_label: SyncRwLock::new("Telegram account".into()),
            own_user_id: AtomicI64::new(0),
            sender_names: RwLock::new(HashMap::new()),
            chat_kinds: RwLock::new(HashMap::new()),
            catalog_loaded: AtomicBool::new(false),
            search_generation: AtomicU64::new(0),
            version_compatible: AtomicBool::new(false),
            catalog_load_lock: Mutex::new(()),
            chat_summary_cache: RwLock::new(None),
            chat_summary_load_lock: Mutex::new(()),
            dirty_chat_summaries: Mutex::new(HashSet::new()),
            group_chat_ids: RwLock::new(HashMap::new()),
            catalog_phase: AtomicU8::new(CATALOG_IDLE),
            catalog_total: AtomicUsize::new(0),
            catalog_processed: AtomicUsize::new(0),
        });
        Self::start_update_loop(&gateway);
        let initial = Arc::clone(&gateway);
        tauri::async_runtime::spawn(async move {
            let version = initial
                .client
                .request(json!({ "@type": "getOption", "name": "version" }))
                .await;
            match version {
                Ok(value)
                    if value.get("value").and_then(Value::as_str)
                        == Some(SUPPORTED_TDLIB_VERSION) =>
                {
                    initial.version_compatible.store(true, Ordering::Release);
                }
                Ok(value) => {
                    let found = value
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    initial.set_auth(
                        AuthStage::Error,
                        Some(&format!(
                            "Unsupported TDLib version {found}; Retract requires {SUPPORTED_TDLIB_VERSION}."
                        )),
                        None,
                    );
                    return;
                }
                Err(error) => {
                    initial.set_auth_error(error);
                    return;
                }
            }
            match initial
                .client
                .request(json!({ "@type": "getAuthorizationState" }))
                .await
            {
                Ok(state) => initial.process_authorization_state(&state).await,
                Err(error) => initial.set_auth_error(error),
            }
        });
        Ok(gateway)
    }

    fn start_update_loop(gateway: &Arc<Self>) {
        let mut updates = gateway.client.subscribe();
        let weak = Arc::downgrade(gateway);
        tauri::async_runtime::spawn(async move {
            loop {
                let update = match updates.recv().await {
                    Ok(update) => update,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let Some(gateway) = weak.upgrade() else { break };
                let update_type = update.get("@type").and_then(Value::as_str);
                if update_type == Some("updateAuthorizationState")
                    && let Some(state) = update.get("authorization_state")
                {
                    gateway.process_authorization_state(state).await;
                } else if matches!(
                    update_type,
                    Some(
                        "updateNewChat"
                            | "updateChatTitle"
                            | "updateChatPosition"
                            | "updateChatLastMessage"
                            | "updateChatAddedToList"
                            | "updateChatRemovedFromList"
                            | "updateChatPermissions"
                            | "updateDeleteMessages"
                    )
                ) {
                    let chat_id = if update_type == Some("updateNewChat") {
                        value_i64(update.pointer("/chat/id"))
                    } else {
                        value_i64(update.get("chat_id"))
                    };
                    if let Some(chat_id) = chat_id {
                        gateway.mark_chat_summary_dirty(chat_id).await;
                    }
                } else if update_type == Some("updateBasicGroup") {
                    if let Some(group_id) = value_i64(update.pointer("/basic_group/id")) {
                        gateway.mark_group_summary_dirty(false, group_id).await;
                    }
                } else if update_type == Some("updateSupergroup")
                    && let Some(group_id) = value_i64(update.pointer("/supergroup/id"))
                {
                    gateway.mark_group_summary_dirty(true, group_id).await;
                }
            }
        });
    }

    async fn process_authorization_state(&self, state: &Value) {
        if !self.version_compatible.load(Ordering::Acquire) {
            return;
        }
        let kind = state
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match kind {
            "authorizationStateWaitTdlibParameters" => {
                self.set_auth(AuthStage::Initializing, Some("Opening the encrypted local message database…"), None);
                let parameters = json!({
                    "@type": "setTdlibParameters",
                    "use_test_dc": self.config.use_test_dc,
                    "database_directory": self.config.data_directory.join("database").to_string_lossy(),
                    "files_directory": self.config.data_directory.join("files").to_string_lossy(),
                    "database_encryption_key": BASE64.encode(self.config.database_key.as_ref()),
                    "use_file_database": true,
                    "use_chat_info_database": true,
                    "use_message_database": true,
                    "use_secret_chats": true,
                    "api_id": self.config.api_id,
                    "api_hash": self.config.api_hash.as_str(),
                    "system_language_code": "en-US",
                    "device_model": "Mac",
                    "system_version": std::env::consts::OS,
                    "application_version": env!("CARGO_PKG_VERSION")
                });
                if let Err(error) = self.client.request(parameters).await {
                    self.set_auth_error(error);
                }
            }
            "authorizationStateWaitPhoneNumber" => self.set_auth(
                AuthStage::WaitingForPhone,
                Some("Scan a QR code with Telegram on another device, or enter your phone number."),
                None,
            ),
            "authorizationStateWaitEmailAddress" => self.set_auth(
                AuthStage::WaitingForEmailAddress,
                Some("Telegram requires an email address for this sign-in."),
                None,
            ),
            "authorizationStateWaitEmailCode" => self.set_auth(
                AuthStage::WaitingForEmailCode,
                state.get("code_info").and_then(|info| info.get("email_address_pattern")).and_then(Value::as_str),
                None,
            ),
            "authorizationStateWaitCode" => self.set_auth(
                AuthStage::WaitingForCode,
                Some("Enter the sign-in code Telegram sent to your other session, SMS, or email."),
                None,
            ),
            "authorizationStateWaitPassword" => self.set_auth(
                AuthStage::WaitingForPassword,
                state.get("password_hint").and_then(Value::as_str),
                None,
            ),
            "authorizationStateWaitOtherDeviceConfirmation" => self.set_auth(
                AuthStage::WaitingForOtherDevice,
                Some("Scan this code from Telegram → Settings → Devices → Link Desktop Device."),
                state.get("link").and_then(Value::as_str),
            ),
            "authorizationStateReady" => {
                self.set_auth(AuthStage::Ready, None, None);
                self.catalog_loaded.store(false, Ordering::Release);
                self.invalidate_chat_summary_cache().await;
                if let Ok(me) = self.client.request(json!({ "@type": "getMe" })).await {
                    if let Some(id) = value_i64(me.get("id")) {
                        self.own_user_id.store(id, Ordering::Release);
                    }
                    let name = display_user_name(&me);
                    if !name.is_empty() && let Ok(mut label) = self.account_label.write() {
                        *label = name;
                    }
                }
            }
            "authorizationStateLoggingOut" | "authorizationStateClosing" => self.set_auth(
                AuthStage::LoggingOut,
                Some("Closing the encrypted Telegram session…"),
                None,
            ),
            "authorizationStateClosed" => self.set_auth(AuthStage::Closed, Some("The Telegram session is closed."), None),
            "authorizationStateWaitRegistration" => self.set_auth(
                AuthStage::Error,
                Some("Retract only signs in to existing accounts; finish registration in an official Telegram app."),
                None,
            ),
            "authorizationStateWaitPremiumPurchase" => self.set_auth(
                AuthStage::Error,
                Some("Telegram requires an account action that Retract cannot complete. Use an official client, then retry."),
                None,
            ),
            _ => self.set_auth(AuthStage::Error, Some("TDLib returned an unsupported authorization state."), None),
        }
    }

    fn set_auth(&self, stage: AuthStage, hint: Option<&str>, qr_link: Option<&str>) {
        if let Ok(mut auth) = self.auth.write() {
            *auth = AuthSnapshot {
                stage,
                hint: hint.map(str::to_owned),
                qr_link: qr_link.map(str::to_owned),
            };
        }
    }

    fn set_auth_error(&self, error: AppError) {
        let message = match error {
            AppError::Gateway(message) => format!("Telegram sign-in failed: {message}"),
            _ => "The local Telegram engine could not be initialized.".into(),
        };
        self.set_auth(AuthStage::Error, Some(&message), None);
    }

    fn is_ready(&self) -> bool {
        self.auth
            .read()
            .is_ok_and(|auth| auth.stage == AuthStage::Ready)
    }

    fn ensure_ready(&self) -> Result<(), AppError> {
        self.is_ready()
            .then_some(())
            .ok_or_else(|| AppError::Gateway("AUTHORIZATION_REQUIRED".into()))
    }

    async fn load_catalog(&self) -> Result<(), AppError> {
        if self.catalog_loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.catalog_load_lock.lock().await;
        if self.catalog_loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        futures_util::future::try_join(
            self.load_chat_list(json!({ "@type": "chatListMain" })),
            self.load_chat_list(json!({ "@type": "chatListArchive" })),
        )
        .await?;
        self.catalog_loaded.store(true, Ordering::Release);
        Ok(())
    }

    async fn load_chat_list(&self, chat_list: Value) -> Result<(), AppError> {
        for _ in 0..100 {
            match self
                .client
                .request(json!({
                    "@type": "loadChats",
                    "chat_list": chat_list,
                    "limit": 100
                }))
                .await
            {
                Ok(_) => continue,
                Err(AppError::Gateway(message)) if message.starts_with("404 ") => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn chat_summaries(&self) -> Result<Vec<ChatSummary>, AppError> {
        if self.chat_summary_cache.read().await.is_some() {
            self.refresh_dirty_chat_summaries().await?;
            if let Some(cached) = self.chat_summary_cache.read().await.as_ref() {
                return Ok(cached.clone());
            }
        }
        let _guard = self.chat_summary_load_lock.lock().await;
        if self.chat_summary_cache.read().await.is_some() {
            self.refresh_dirty_chat_summaries().await?;
            if let Some(cached) = self.chat_summary_cache.read().await.as_ref() {
                return Ok(cached.clone());
            }
        }
        self.dirty_chat_summaries.lock().await.clear();
        self.group_chat_ids.write().await.clear();
        self.catalog_total.store(0, Ordering::Release);
        self.catalog_processed.store(0, Ordering::Release);
        self.catalog_phase
            .store(CATALOG_DISCOVERING, Ordering::Release);
        self.load_catalog().await?;
        let mut ids = Vec::new();
        for list in [Value::Null, json!({ "@type": "chatListArchive" })] {
            let response = self
                .client
                .request(json!({
                    "@type": "getChats",
                    "chat_list": list,
                    "limit": 10000
                }))
                .await?;
            for id in response
                .get("chat_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = value_i64(Some(id)) {
                    ids.push(id)
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        self.catalog_total.store(ids.len(), Ordering::Release);
        self.catalog_phase.store(CATALOG_LOADING, Ordering::Release);
        let mut summaries = stream::iter(ids)
            .map(|id| async move {
                let chat = self
                    .client
                    .request(json!({ "@type": "getChat", "chat_id": id }))
                    .await?;
                let summary = self.map_chat(&chat).await?;
                self.catalog_processed.fetch_add(1, Ordering::AcqRel);
                Ok::<ChatSummary, AppError>(summary)
            })
            .buffer_unordered(CHAT_SUMMARY_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        let mut kind_cache = HashMap::new();
        for summary in &summaries {
            kind_cache.insert(summary.id, summary.kind);
        }
        summaries.sort_by_key(|chat| chat.title.to_lowercase());
        *self.chat_kinds.write().await = kind_cache;
        *self.chat_summary_cache.write().await = Some(summaries.clone());
        self.catalog_phase.store(CATALOG_READY, Ordering::Release);
        Ok(summaries)
    }

    async fn invalidate_chat_summary_cache(&self) {
        *self.chat_summary_cache.write().await = None;
        self.dirty_chat_summaries.lock().await.clear();
        self.catalog_total.store(0, Ordering::Release);
        self.catalog_processed.store(0, Ordering::Release);
        self.catalog_phase.store(CATALOG_IDLE, Ordering::Release);
    }

    fn catalog_progress_snapshot(&self) -> CatalogProgress {
        let phase = match self.catalog_phase.load(Ordering::Acquire) {
            CATALOG_DISCOVERING => "discovering",
            CATALOG_LOADING => "loading",
            CATALOG_READY => "ready",
            _ => "idle",
        };
        CatalogProgress {
            phase,
            total: self.catalog_total.load(Ordering::Acquire),
            processed: self.catalog_processed.load(Ordering::Acquire),
        }
    }

    async fn mark_chat_summary_dirty(&self, chat_id: i64) {
        if self.chat_summary_cache.read().await.is_some() {
            self.dirty_chat_summaries.lock().await.insert(chat_id);
        }
    }

    async fn mark_group_summary_dirty(&self, supergroup: bool, group_id: i64) {
        let mut chat_id = self
            .group_chat_ids
            .read()
            .await
            .get(&(supergroup, group_id))
            .copied();
        if chat_id.is_none() {
            let request = if supergroup {
                json!({
                    "@type": "createSupergroupChat",
                    "supergroup_id": group_id,
                    "force": false
                })
            } else {
                json!({
                    "@type": "createBasicGroupChat",
                    "basic_group_id": group_id,
                    "force": false
                })
            };
            chat_id = self
                .client
                .request(request)
                .await
                .ok()
                .and_then(|chat| value_i64(chat.get("id")));
            if let Some(chat_id) = chat_id {
                self.group_chat_ids
                    .write()
                    .await
                    .insert((supergroup, group_id), chat_id);
            }
        }
        if let Some(chat_id) = chat_id {
            self.mark_chat_summary_dirty(chat_id).await;
        } else if self.chat_summary_cache.read().await.is_some() {
            // Preserve correctness if TDLib cannot resolve an unexpected group.
            // The normal path above repairs the one missing group mapping without
            // rebuilding the catalog.
            self.invalidate_chat_summary_cache().await;
        }
    }

    async fn apply_chat_summary_updates(&self, updates: Vec<(i64, Option<ChatSummary>)>) {
        let mut cache = self.chat_summary_cache.write().await;
        let Some(summaries) = cache.as_mut() else {
            drop(cache);
            let mut kinds = self.chat_kinds.write().await;
            for (chat_id, replacement) in updates {
                if let Some(summary) = replacement {
                    kinds.insert(chat_id, summary.kind);
                } else {
                    kinds.remove(&chat_id);
                }
            }
            return;
        };
        for (chat_id, replacement) in updates {
            summaries.retain(|chat| chat.id != chat_id);
            if let Some(summary) = replacement {
                summaries.push(summary);
            }
        }
        summaries.sort_by_key(|chat| chat.title.to_lowercase());
        let kind_cache = summaries.iter().map(|chat| (chat.id, chat.kind)).collect();
        let count = summaries.len();
        drop(cache);
        *self.chat_kinds.write().await = kind_cache;
        self.catalog_total.store(count, Ordering::Release);
        self.catalog_processed.store(count, Ordering::Release);
        self.catalog_phase.store(CATALOG_READY, Ordering::Release);
    }

    async fn refresh_dirty_chat_summaries(&self) -> Result<(), AppError> {
        let ids: Vec<_> = self.dirty_chat_summaries.lock().await.drain().collect();
        if ids.is_empty() {
            return Ok(());
        }

        let retry_ids = ids.clone();
        let updates = stream::iter(ids)
            .map(|chat_id| async move {
                let chat = match self
                    .client
                    .request(json!({ "@type": "getChat", "chat_id": chat_id }))
                    .await
                {
                    Ok(chat) => chat,
                    Err(AppError::Gateway(message)) if message.starts_with("404 ") => {
                        return Ok((chat_id, None));
                    }
                    Err(error) => return Err(error),
                };
                if !chat_is_in_catalog(&chat) {
                    return Ok((chat_id, None));
                }
                self.map_chat(&chat)
                    .await
                    .map(|summary| (chat_id, Some(summary)))
            })
            .buffer_unordered(CHAT_SUMMARY_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await;
        let updates = match updates {
            Ok(updates) => updates,
            Err(error) => {
                self.dirty_chat_summaries.lock().await.extend(retry_ids);
                return Err(error);
            }
        };

        self.apply_chat_summary_updates(updates).await;
        Ok(())
    }

    async fn map_chat(&self, chat: &Value) -> Result<ChatSummary, AppError> {
        let id = required_i64(chat.get("id"), "chat.id")?;
        let raw_title = chat
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled chat")
            .to_owned();
        let archived = chat
            .get("positions")
            .and_then(Value::as_array)
            .is_some_and(|positions| {
                positions.iter().any(|position| {
                    position.pointer("/list/@type").and_then(Value::as_str)
                        == Some("chatListArchive")
                })
            });
        let type_value = chat
            .get("type")
            .ok_or_else(|| AppError::Gateway("TDLIB_CHAT_TYPE_MISSING".into()))?;
        let type_name = type_value
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let clearable = chat
            .get("can_be_deleted_for_all_users")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let removable_for_self = chat
            .get("can_be_deleted_only_for_self")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut group_lookup = None;
        let (kind, member_count, role, can_delete_others, can_leave_chat) = match type_name {
            "chatTypePrivate" => (ChatKind::Direct, None, ChatRole::Member, false, false),
            "chatTypeSecret" => (ChatKind::Secret, None, ChatRole::Member, false, false),
            "chatTypeBasicGroup" => {
                let group_id = required_i64(type_value.get("basic_group_id"), "basic_group_id")?;
                group_lookup = Some((false, group_id));
                let group = self
                    .client
                    .request(json!({ "@type": "getBasicGroup", "basic_group_id": group_id }))
                    .await?;
                let (role, delete, can_leave) = role_from_status(group.get("status"));
                (
                    ChatKind::BasicGroup,
                    value_u32(group.get("member_count")),
                    role,
                    delete,
                    can_leave,
                )
            }
            "chatTypeSupergroup" => {
                let group_id = required_i64(type_value.get("supergroup_id"), "supergroup_id")?;
                group_lookup = Some((true, group_id));
                let group = self
                    .client
                    .request(json!({ "@type": "getSupergroup", "supergroup_id": group_id }))
                    .await?;
                let is_channel = group
                    .get("is_channel")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let (role, delete, can_leave) = role_from_status(group.get("status"));
                (
                    if is_channel {
                        ChatKind::Channel
                    } else {
                        ChatKind::Supergroup
                    },
                    value_u32(group.get("member_count")),
                    role,
                    delete,
                    can_leave,
                )
            }
            _ => return Err(AppError::Gateway("TDLIB_UNKNOWN_CHAT_TYPE".into())),
        };
        let title = if raw_title.trim().is_empty() {
            match kind {
                ChatKind::Direct => "Deleted account".into(),
                ChatKind::Secret => "Deleted secret chat".into(),
                _ => "Untitled chat".into(),
            }
        } else {
            raw_title
        };
        if let Some(group_lookup) = group_lookup {
            self.group_chat_ids.write().await.insert(group_lookup, id);
        }
        let can_delete_group = clearable
            && role == ChatRole::Owner
            && matches!(kind, ChatKind::BasicGroup | ChatKind::Supergroup);
        let can_delete_by_sender = kind == ChatKind::Supergroup && can_delete_others;
        let conversation_state = self.conversation_state(chat, id, kind, role).await;
        Ok(ChatSummary {
            id,
            title,
            kind,
            archived,
            member_count,
            conversation_state,
            capabilities: ChatCapabilities {
                role,
                can_delete_others,
                can_clear_for_everyone: clearable,
                can_remove_for_self: removable_for_self,
                can_delete_group,
                can_delete_by_sender,
                can_leave_chat,
            },
            avatar_seed: id.unsigned_abs().wrapping_mul(31) as u8,
        })
    }

    async fn conversation_state(
        &self,
        chat: &Value,
        chat_id: i64,
        kind: ChatKind,
        role: ChatRole,
    ) -> ConversationState {
        if kind == ChatKind::Channel {
            return ConversationState::Unknown;
        }

        let last_message = if let Some(message) = chat
            .get("last_message")
            .filter(|message| !message.is_null())
        {
            Some(message.clone())
        } else {
            let history = match self
                .client
                .request(json!({
                    "@type": "getChatHistory",
                    "chat_id": chat_id,
                    "from_message_id": 0,
                    "offset": 0,
                    "limit": 1,
                    "only_local": false
                }))
                .await
            {
                Ok(history) => history,
                Err(_) => return ConversationState::Unknown,
            };
            history
                .get("messages")
                .and_then(Value::as_array)
                .and_then(|messages| messages.iter().find(|message| !message.is_null()))
                .cloned()
        };

        let Some(last_message) = last_message else {
            return ConversationState::Empty;
        };
        if last_message
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return ConversationState::Active;
        }

        // TDLib doesn't support sender-filtered searches in secret chats. The latest
        // incoming message is still safe to label as awaiting a reply, but we don't
        // infer that the account has never replied.
        if kind == ChatKind::Secret {
            return ConversationState::AwaitingReply;
        }
        let own_user_id = self.own_user_id.load(Ordering::Acquire);
        if own_user_id <= 0 {
            return ConversationState::AwaitingReply;
        }
        let sent_by_user = match self
            .client
            .request(json!({
                "@type": "searchChatMessages",
                "chat_id": chat_id,
                "topic_id": null,
                "query": "",
                "sender_id": { "@type": "messageSenderUser", "user_id": own_user_id },
                "from_message_id": 0,
                "offset": 0,
                "limit": 1,
                "filter": null
            }))
            .await
        {
            Ok(response) => {
                response
                    .get("total_count")
                    .and_then(Value::as_i64)
                    .is_some_and(|count| count > 0)
                    || response
                        .get("messages")
                        .and_then(Value::as_array)
                        .is_some_and(|messages| messages.iter().any(|message| !message.is_null()))
            }
            Err(_) => return ConversationState::AwaitingReply,
        };
        if sent_by_user {
            ConversationState::AwaitingReply
        } else if role == ChatRole::Member || kind == ChatKind::Direct {
            ConversationState::NeverReplied
        } else {
            // Admins can post as the group/channel identity. A zero-result user-sender
            // search can't prove they never replied, so keep it out of the shortlist.
            ConversationState::Unknown
        }
    }

    async fn raw_search(&self, request: &SearchRequest) -> Result<Vec<Value>, AppError> {
        let server_filter = search_filter(&request.content_kinds);
        if !request.chat_ids.is_empty() {
            let kinds = self.chat_kinds.read().await.clone();
            let mut all = Vec::new();
            for chat_id in &request.chat_ids {
                let secret = kinds.get(chat_id) == Some(&ChatKind::Secret);
                if secret && !request.query.is_empty() {
                    all.extend(
                        self.search_secret(*chat_id, request, server_filter.clone())
                            .await?,
                    );
                } else {
                    all.extend(
                        self.search_chat(*chat_id, request, server_filter.clone())
                            .await?,
                    );
                }
                if all.len() >= request.limit.saturating_mul(2) {
                    break;
                }
            }
            return Ok(all);
        }
        if request.query.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = self.search_global(request, server_filter.clone()).await?;
        all.extend(self.search_secret(0, request, server_filter).await?);
        Ok(all)
    }

    async fn search_global(
        &self,
        request: &SearchRequest,
        filter: Value,
    ) -> Result<Vec<Value>, AppError> {
        let mut messages = Vec::new();
        let mut offset = String::new();
        while messages.len() < request.limit.saturating_mul(2).max(100) {
            let response = self
                .client
                .request(json!({
                    "@type": "searchMessages",
                    "chat_list": null,
                    "query": request.query,
                    "offset": offset,
                    "limit": 100,
                    "filter": filter,
                    "chat_type_filter": null,
                    "min_date": timestamp_i32(request.min_date),
                    "max_date": timestamp_i32(request.max_date)
                }))
                .await?;
            append_raw_messages(&mut messages, &response);
            let next = response
                .get("next_offset")
                .and_then(Value::as_str)
                .unwrap_or("");
            if next.is_empty() || next == offset {
                break;
            }
            offset = next.to_owned();
        }
        Ok(messages)
    }

    async fn search_secret(
        &self,
        chat_id: i64,
        request: &SearchRequest,
        filter: Value,
    ) -> Result<Vec<Value>, AppError> {
        let mut messages = Vec::new();
        let mut offset = String::new();
        while messages.len() < request.limit.saturating_mul(2).max(100) {
            let response = self
                .client
                .request(json!({
                    "@type": "searchSecretMessages",
                    "chat_id": chat_id,
                    "query": request.query,
                    "offset": offset,
                    "limit": 100,
                    "filter": filter
                }))
                .await?;
            append_raw_messages(&mut messages, &response);
            let next = response
                .get("next_offset")
                .and_then(Value::as_str)
                .unwrap_or("");
            if next.is_empty() || next == offset {
                break;
            }
            offset = next.to_owned();
        }
        Ok(messages)
    }

    async fn search_chat(
        &self,
        chat_id: i64,
        request: &SearchRequest,
        filter: Value,
    ) -> Result<Vec<Value>, AppError> {
        let mut messages = Vec::new();
        let mut from_message_id = 0_i64;
        while messages.len() < request.limit.saturating_mul(2).max(100) {
            let response = self
                .client
                .request(json!({
                    "@type": "searchChatMessages",
                    "chat_id": chat_id,
                    "topic_id": null,
                    "query": request.query,
                    "sender_id": null,
                    "from_message_id": from_message_id,
                    "offset": 0,
                    "limit": 100,
                    "filter": filter
                }))
                .await?;
            append_raw_messages(&mut messages, &response);
            let next = value_i64(response.get("next_from_message_id")).unwrap_or(0);
            if next == 0 || next == from_message_id {
                break;
            }
            from_message_id = next;
        }
        Ok(messages)
    }

    async fn raw_own_messages(&self, chat_id: i64) -> Result<Vec<Value>, AppError> {
        let mut own_user_id = self.own_user_id.load(Ordering::Acquire);
        if own_user_id <= 0 {
            let me = self.client.request(json!({ "@type": "getMe" })).await?;
            own_user_id = required_i64(me.get("id"), "user.id")?;
            self.own_user_id.store(own_user_id, Ordering::Release);
        }

        let mut messages = Vec::new();
        let mut seen = HashSet::new();
        let mut from_message_id = 0_i64;
        while messages.len() < CHAT_CLEANUP_MESSAGE_LIMIT {
            let response = self
                .client
                .request(json!({
                    "@type": "searchChatMessages",
                    "chat_id": chat_id,
                    "topic_id": null,
                    "query": "",
                    "sender_id": { "@type": "messageSenderUser", "user_id": own_user_id },
                    "from_message_id": from_message_id,
                    "offset": 0,
                    "limit": 100,
                    "filter": null
                }))
                .await?;
            for message in response
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| !message.is_null())
            {
                let message_id = value_i64(message.get("id")).unwrap_or_default();
                if seen.insert(message_id) {
                    messages.push(message.clone());
                    if messages.len() >= CHAT_CLEANUP_MESSAGE_LIMIT {
                        break;
                    }
                }
            }
            let next = value_i64(response.get("next_from_message_id")).unwrap_or(0);
            if next == 0 || next == from_message_id {
                break;
            }
            from_message_id = next;
        }
        Ok(messages)
    }

    async fn raw_chat_messages(&self, chat_id: i64) -> Result<Vec<Value>, AppError> {
        let mut messages = Vec::new();
        let mut seen = HashSet::new();
        let mut from_message_id = 0_i64;
        while messages.len() < CHAT_CLEANUP_MESSAGE_LIMIT {
            let response = self
                .client
                .request(json!({
                    "@type": "searchChatMessages",
                    "chat_id": chat_id,
                    "topic_id": null,
                    "query": "",
                    "sender_id": null,
                    "from_message_id": from_message_id,
                    "offset": 0,
                    "limit": 100,
                    "filter": null
                }))
                .await?;
            for message in response
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| !message.is_null())
            {
                let message_id = value_i64(message.get("id")).unwrap_or_default();
                if seen.insert(message_id) {
                    messages.push(message.clone());
                    if messages.len() >= CHAT_CLEANUP_MESSAGE_LIMIT {
                        break;
                    }
                }
            }
            let next = value_i64(response.get("next_from_message_id")).unwrap_or(0);
            if next == 0 || next == from_message_id {
                break;
            }
            from_message_id = next;
        }
        Ok(messages)
    }

    async fn map_message(&self, message: &Value) -> Result<MessageSnapshot, AppError> {
        let chat_id = required_i64(message.get("chat_id"), "message.chat_id")?;
        let message_id = required_i64(message.get("id"), "message.id")?;
        let is_outgoing = message
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (sender_id, sender_is_chat) = message
            .get("sender_id")
            .map(message_sender_id)
            .unwrap_or((0, false));
        let sender_name = if is_outgoing {
            "You".into()
        } else {
            self.sender_name(sender_id, sender_is_chat).await
        };
        let content = message.get("content").unwrap_or(&Value::Null);
        let (content_kind, preview) = content_preview(content);
        let properties = self
            .client
            .request(json!({
                "@type": "getMessageProperties",
                "chat_id": chat_id,
                "message_id": message_id
            }))
            .await
            .ok();
        let deletion_reach = properties
            .as_ref()
            .map_or(DeletionReach::None, |properties| {
                if properties
                    .get("can_be_deleted_for_all_users")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    DeletionReach::Everyone
                } else if properties
                    .get("can_be_deleted_only_for_self")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    DeletionReach::SelfOnly
                } else {
                    DeletionReach::None
                }
            });
        let date = value_i64(message.get("date")).unwrap_or_default();
        let sent_at = DateTime::from_timestamp(date, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        Ok(MessageSnapshot {
            chat_id,
            message_id,
            sender_id,
            sender_name,
            sent_at,
            is_outgoing,
            content_kind,
            preview,
            privacy_findings: Vec::new(),
            album_id: value_i64(message.get("media_album_id")).filter(|id| *id != 0),
            is_pinned: message
                .get("is_pinned")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            deletion_reach,
        })
    }

    async fn sender_name(&self, id: i64, is_chat: bool) -> String {
        if id == 0 {
            return "Unknown sender".into();
        }
        if let Some(name) = self.sender_names.read().await.get(&(is_chat, id)).cloned() {
            return name;
        }
        let response = if is_chat {
            self.client
                .request(json!({ "@type": "getChat", "chat_id": id }))
                .await
        } else {
            self.client
                .request(json!({ "@type": "getUser", "user_id": id }))
                .await
        };
        let name = match response {
            Ok(value) if is_chat => value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Unknown chat")
                .to_owned(),
            Ok(value) => {
                let name = display_user_name(&value);
                if name.is_empty() {
                    "Unknown sender".into()
                } else {
                    name
                }
            }
            Err(_) => "Unknown sender".into(),
        };
        self.sender_names
            .write()
            .await
            .insert((is_chat, id), name.clone());
        name
    }

    async fn privacy_search(
        &self,
        request: &SearchRequest,
        generation: u64,
    ) -> Result<Vec<MessageSnapshot>, AppError> {
        let kinds = self.chat_kinds.read().await.clone();
        let mut chat_ids = if request.chat_ids.is_empty() {
            kinds.keys().copied().collect::<Vec<_>>()
        } else {
            request.chat_ids.clone()
        };
        chat_ids.sort_unstable();
        chat_ids.dedup();
        let query_tokens: Vec<_> = request
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let server_filter = search_filter(&request.content_kinds);
        let mut matches = Vec::new();

        for chat_id in chat_ids {
            if self.search_generation.load(Ordering::Acquire) != generation {
                return Ok(Vec::new());
            }
            if !request.chat_kinds.is_empty()
                && !kinds
                    .get(&chat_id)
                    .is_some_and(|kind| request.chat_kinds.contains(kind))
            {
                continue;
            }
            let mut from_message_id = 0_i64;
            loop {
                if self.search_generation.load(Ordering::Acquire) != generation {
                    return Ok(Vec::new());
                }
                let response = match self
                    .client
                    .request(json!({
                        "@type": "searchChatMessages",
                        "chat_id": chat_id,
                        "topic_id": null,
                        "query": "",
                        "sender_id": null,
                        "from_message_id": from_message_id,
                        "offset": 0,
                        "limit": 100,
                        "filter": server_filter
                    }))
                    .await
                {
                    Ok(response) => response,
                    Err(AppError::Gateway(message)) if message.starts_with("404 ") => break,
                    Err(error) => return Err(error),
                };
                let raw_messages = response
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if raw_messages.is_empty() {
                    break;
                }
                let mut saw_new_message = false;
                for raw in &raw_messages {
                    let message_id = value_i64(raw.get("id")).unwrap_or_default();
                    if message_id == 0 || (from_message_id != 0 && message_id == from_message_id) {
                        continue;
                    }
                    saw_new_message = true;
                    let is_outgoing = raw
                        .get("is_outgoing")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if matches!(request.direction, MessageDirection::Mine) && !is_outgoing
                        || matches!(request.direction, MessageDirection::Others) && is_outgoing
                    {
                        continue;
                    }
                    if request.exclude_pinned
                        && raw
                            .get("is_pinned")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    {
                        continue;
                    }
                    let date = value_i64(raw.get("date")).unwrap_or_default();
                    let sent_at =
                        DateTime::from_timestamp(date, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
                    if request.min_date.is_some_and(|minimum| sent_at < minimum)
                        || request.max_date.is_some_and(|maximum| sent_at > maximum)
                    {
                        continue;
                    }
                    let content = raw.get("content").unwrap_or(&Value::Null);
                    let (content_kind, _) = content_preview(content);
                    if !request.content_kinds.is_empty()
                        && !request.content_kinds.contains(&content_kind)
                    {
                        continue;
                    }
                    let findings =
                        detect_sensitive_data(&sensitive_content_text(content), content_kind);
                    if findings.is_empty() {
                        continue;
                    }
                    let mut message = self.map_message(raw).await?;
                    let searchable =
                        format!("{} {}", message.preview, message.sender_name).to_lowercase();
                    if !query_tokens.iter().all(|token| searchable.contains(token)) {
                        continue;
                    }
                    message.privacy_findings = findings;
                    matches.push(message);
                    if matches.len() > request.limit.saturating_mul(2) {
                        matches.sort_by_key(|message| std::cmp::Reverse(message.sent_at));
                        matches.truncate(request.limit);
                    }
                }
                let next = value_i64(response.get("next_from_message_id")).unwrap_or(0);
                if next == 0 || next == from_message_id || !saw_new_message {
                    break;
                }
                if request.min_date.is_some_and(|minimum| {
                    raw_messages.iter().all(|message| {
                        value_i64(message.get("date"))
                            .and_then(|date| DateTime::from_timestamp(date, 0))
                            .is_some_and(|date| date < minimum)
                    })
                }) {
                    break;
                }
                from_message_id = next;
            }
        }
        matches.sort_by_key(|message| std::cmp::Reverse(message.sent_at));
        matches.truncate(request.limit);
        Ok(matches)
    }

    async fn submit_auth_request(&self, stage: AuthStage, request: Value) -> Result<(), AppError> {
        let current = self.auth();
        if current.stage != stage {
            return Err(AppError::InvalidRequest(
                "the authentication step changed; refresh and try again".into(),
            ));
        }
        self.client.request(request).await.map(|_| ())
    }
}

#[async_trait]
impl TelegramGateway for LiveGateway {
    fn info(&self) -> GatewayInfo {
        GatewayInfo {
            mode: "live",
            account_label: self
                .account_label
                .read()
                .map(|value| value.clone())
                .unwrap_or_else(|_| "Telegram account".into()),
            reason: None,
        }
    }

    fn auth(&self) -> AuthSnapshot {
        self.auth
            .read()
            .map(|value| value.clone())
            .unwrap_or(AuthSnapshot {
                stage: AuthStage::Error,
                hint: Some("Authentication state is unavailable.".into()),
                qr_link: None,
            })
    }

    fn catalog_progress(&self) -> CatalogProgress {
        self.catalog_progress_snapshot()
    }

    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError> {
        if !self.is_ready() {
            return Ok(Vec::new());
        }
        self.chat_summaries().await
    }

    async fn chat_by_id(&self, chat_id: i64) -> Result<Option<ChatSummary>, AppError> {
        self.ensure_ready()?;
        let chat = match self
            .client
            .request(json!({ "@type": "getChat", "chat_id": chat_id }))
            .await
        {
            Ok(chat) => chat,
            Err(AppError::Gateway(message)) if message.starts_with("404 ") => {
                self.apply_chat_summary_updates(vec![(chat_id, None)]).await;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if !chat_is_in_catalog(&chat) {
            self.apply_chat_summary_updates(vec![(chat_id, None)]).await;
            return Ok(None);
        }
        let summary = self.map_chat(&chat).await?;
        self.apply_chat_summary_updates(vec![(chat_id, Some(summary.clone()))])
            .await;
        self.dirty_chat_summaries.lock().await.remove(&chat_id);
        Ok(Some(summary))
    }

    async fn search(&self, request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError> {
        self.ensure_ready()?;
        let generation = self
            .search_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if request.privacy_scan {
            return self.privacy_search(request, generation).await;
        }
        let kinds = self.chat_kinds.read().await.clone();
        let mut mapped = Vec::new();
        let mut seen = HashSet::new();
        for raw in self.raw_search(request).await? {
            let chat_id = value_i64(raw.get("chat_id")).unwrap_or_default();
            let message_id = value_i64(raw.get("id")).unwrap_or_default();
            if !seen.insert((chat_id, message_id)) {
                continue;
            }
            let message = self.map_message(&raw).await?;
            if !request.chat_kinds.is_empty()
                && !kinds
                    .get(&message.chat_id)
                    .is_some_and(|kind| request.chat_kinds.contains(kind))
            {
                continue;
            }
            if !request.content_kinds.is_empty()
                && !request.content_kinds.contains(&message.content_kind)
            {
                continue;
            }
            if matches!(request.direction, MessageDirection::Mine) && !message.is_outgoing {
                continue;
            }
            if matches!(request.direction, MessageDirection::Others) && message.is_outgoing {
                continue;
            }
            if request.exclude_pinned && message.is_pinned {
                continue;
            }
            if request.min_date.is_some_and(|date| message.sent_at < date) {
                continue;
            }
            if request.max_date.is_some_and(|date| message.sent_at > date) {
                continue;
            }
            mapped.push(message);
            if mapped.len() >= request.limit {
                break;
            }
        }
        mapped.sort_by_key(|message| std::cmp::Reverse(message.sent_at));
        Ok(mapped)
    }

    async fn own_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        self.ensure_ready()?;
        stream::iter(self.raw_own_messages(chat_id).await?)
            .map(|raw| async move { self.map_message(&raw).await })
            .buffer_unordered(MESSAGE_MAPPING_CONCURRENCY)
            .try_collect()
            .await
    }

    async fn chat_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        self.ensure_ready()?;
        stream::iter(self.raw_chat_messages(chat_id).await?)
            .map(|raw| async move { self.map_message(&raw).await })
            .buffer_unordered(MESSAGE_MAPPING_CONCURRENCY)
            .try_collect()
            .await
    }

    async fn messages_by_ids(&self, ids: &[(i64, i64)]) -> Result<Vec<MessageSnapshot>, AppError> {
        self.ensure_ready()?;
        let mut by_chat: HashMap<i64, Vec<i64>> = HashMap::new();
        for (chat_id, message_id) in ids {
            by_chat.entry(*chat_id).or_default().push(*message_id)
        }
        let mut snapshots = Vec::new();
        for (chat_id, message_ids) in by_chat {
            for chunk in message_ids.chunks(100) {
                let response = self
                    .client
                    .request(json!({
                        "@type": "getMessages",
                        "chat_id": chat_id,
                        "message_ids": chunk
                    }))
                    .await?;
                for message in response
                    .get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if !message.is_null() {
                        snapshots.push(self.map_message(message).await?)
                    }
                }
            }
        }
        Ok(snapshots)
    }

    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError> {
        self.ensure_ready()?;
        let response = match self
            .client
            .request(json!({
                "@type": "getMessageProperties", "chat_id": chat_id, "message_id": message_id
            }))
            .await
        {
            Ok(response) => response,
            Err(AppError::Gateway(message)) if message.starts_with("404 ") => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(
            if response
                .get("can_be_deleted_for_all_users")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                DeletionReach::Everyone
            } else if response
                .get("can_be_deleted_only_for_self")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                DeletionReach::SelfOnly
            } else {
                DeletionReach::None
            },
        ))
    }

    async fn delete_messages_for_everyone(
        &self,
        chat_id: i64,
        message_ids: &[i64],
    ) -> Result<(), AppError> {
        self.ensure_ready()?;
        if message_ids.is_empty() || message_ids.len() > 100 {
            return Err(AppError::InvalidRequest(
                "TDLib deletion batches must contain 1–100 messages".into(),
            ));
        }
        self.client.request(json!({
            "@type": "deleteMessages", "chat_id": chat_id, "message_ids": message_ids, "revoke": true
        })).await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn clear_history_for_everyone(&self, chat_id: i64) -> Result<(), AppError> {
        self.ensure_ready()?;
        self.client
            .request(json!({
                "@type": "deleteChatHistory", "chat_id": chat_id,
                "remove_from_chat_list": true, "revoke": true
            }))
            .await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn clear_history_for_everyone_keep_chat(&self, chat_id: i64) -> Result<(), AppError> {
        self.ensure_ready()?;
        self.client
            .request(json!({
                "@type": "deleteChatHistory", "chat_id": chat_id,
                "remove_from_chat_list": false, "revoke": true
            }))
            .await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn remove_chat_for_self(&self, chat_id: i64) -> Result<(), AppError> {
        self.ensure_ready()?;
        self.client
            .request(json!({
                "@type": "deleteChatHistory", "chat_id": chat_id,
                "remove_from_chat_list": true, "revoke": false
            }))
            .await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn delete_group(&self, chat_id: i64) -> Result<(), AppError> {
        self.ensure_ready()?;
        self.client
            .request(json!({ "@type": "deleteChat", "chat_id": chat_id }))
            .await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn leave_chat(&self, chat_id: i64) -> Result<(), AppError> {
        self.ensure_ready()?;
        self.client
            .request(json!({ "@type": "leaveChat", "chat_id": chat_id }))
            .await?;

        // Leaving changes only this account's membership. Once Telegram applies
        // that change, explicitly remove the local history/list entry when its
        // refreshed capability allows the self-only operation.
        let refreshed = self
            .client
            .request(json!({ "@type": "getChat", "chat_id": chat_id }))
            .await?;
        let removable_for_self = refreshed
            .get("can_be_deleted_only_for_self")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if removable_for_self {
            self.client
                .request(json!({
                    "@type": "deleteChatHistory",
                    "chat_id": chat_id,
                    "remove_from_chat_list": true,
                    "revoke": false
                }))
                .await?;
        }
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn delete_messages_by_sender(
        &self,
        chat_id: i64,
        sender_id: i64,
    ) -> Result<(), AppError> {
        self.ensure_ready()?;
        let sender = if sender_id > 0 {
            json!({ "@type": "messageSenderUser", "user_id": sender_id })
        } else {
            json!({ "@type": "messageSenderChat", "chat_id": sender_id })
        };
        self.client
            .request(json!({
                "@type": "deleteChatMessagesBySender", "chat_id": chat_id, "sender_id": sender
            }))
            .await?;
        self.mark_chat_summary_dirty(chat_id).await;
        Ok(())
    }

    async fn request_qr_auth(&self) -> Result<(), AppError> {
        if !matches!(
            self.auth().stage,
            AuthStage::WaitingForPhone
                | AuthStage::WaitingForCode
                | AuthStage::WaitingForPassword
                | AuthStage::WaitingForEmailAddress
                | AuthStage::WaitingForEmailCode
        ) {
            return Err(AppError::InvalidRequest(
                "QR sign-in is not available at this authentication step".into(),
            ));
        }
        self.client
            .request(json!({ "@type": "requestQrCodeAuthentication", "other_user_ids": [] }))
            .await
            .map(|_| ())
    }

    async fn submit_phone(&self, phone: &str) -> Result<(), AppError> {
        let phone = bounded_secret(phone, 3, 32, "phone number")?;
        self.submit_auth_request(
            AuthStage::WaitingForPhone,
            json!({
                "@type": "setAuthenticationPhoneNumber", "phone_number": phone, "settings": null
            }),
        )
        .await
    }

    async fn submit_email_address(&self, email: &str) -> Result<(), AppError> {
        let email = bounded_secret(email, 3, 254, "email address")?;
        self.submit_auth_request(
            AuthStage::WaitingForEmailAddress,
            json!({
                "@type": "setAuthenticationEmailAddress", "email_address": email
            }),
        )
        .await
    }

    async fn submit_email_code(&self, code: &str) -> Result<(), AppError> {
        let code = bounded_secret(code, 1, 32, "email code")?;
        self.submit_auth_request(
            AuthStage::WaitingForEmailCode,
            json!({
                "@type": "checkAuthenticationEmailCode",
                "code": { "@type": "emailAddressAuthenticationCode", "code": code }
            }),
        )
        .await
    }

    async fn submit_code(&self, code: &str) -> Result<(), AppError> {
        let code = bounded_secret(code, 1, 32, "authentication code")?;
        self.submit_auth_request(
            AuthStage::WaitingForCode,
            json!({
                "@type": "checkAuthenticationCode", "code": code
            }),
        )
        .await
    }

    async fn submit_password(&self, password: &str) -> Result<(), AppError> {
        if password.is_empty() || password.chars().count() > 256 {
            return Err(AppError::InvalidRequest(
                "password length is invalid".into(),
            ));
        }
        self.submit_auth_request(
            AuthStage::WaitingForPassword,
            json!({
                "@type": "checkAuthenticationPassword", "password": password
            }),
        )
        .await
    }

    async fn close(&self) -> Result<(), AppError> {
        if matches!(self.auth().stage, AuthStage::Closed | AuthStage::LoggingOut) {
            return Ok(());
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.client.request(json!({ "@type": "close" })),
        )
        .await
        .map_err(|_| AppError::Gateway("TDLIB_CLOSE_TIMEOUT".into()))??;
        Ok(())
    }

    async fn reset_demo(&self) -> Result<(), AppError> {
        Err(AppError::InvalidRequest(
            "reset is only available in demo mode".into(),
        ))
    }
}

fn append_raw_messages(destination: &mut Vec<Value>, response: &Value) {
    if let Some(messages) = response.get("messages").and_then(Value::as_array) {
        destination.extend(
            messages
                .iter()
                .filter(|message| !message.is_null())
                .cloned(),
        );
    }
}

fn value_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

fn chat_is_in_catalog(chat: &Value) -> bool {
    chat.get("positions")
        .and_then(Value::as_array)
        .is_some_and(|positions| {
            positions.iter().any(|position| {
                matches!(
                    position.pointer("/list/@type").and_then(Value::as_str),
                    Some("chatListMain" | "chatListArchive")
                ) && value_i64(position.get("order")).unwrap_or_default() != 0
            })
        })
}

fn required_i64(value: Option<&Value>, field: &str) -> Result<i64, AppError> {
    value_i64(value).ok_or_else(|| AppError::Gateway(format!("TDLIB_FIELD_MISSING: {field}")))
}

fn value_u32(value: Option<&Value>) -> Option<u32> {
    value_i64(value).and_then(|value| value.try_into().ok())
}

fn message_sender_id(sender: &Value) -> (i64, bool) {
    match sender.get("@type").and_then(Value::as_str) {
        Some("messageSenderChat") => (value_i64(sender.get("chat_id")).unwrap_or_default(), true),
        _ => (value_i64(sender.get("user_id")).unwrap_or_default(), false),
    }
}

fn role_from_status(status: Option<&Value>) -> (ChatRole, bool, bool) {
    let Some(status) = status else {
        return (ChatRole::Member, false, false);
    };
    match status.get("@type").and_then(Value::as_str) {
        Some("chatMemberStatusCreator") => (ChatRole::Owner, true, false),
        Some("chatMemberStatusAdministrator") => {
            let can_delete = status
                .pointer("/rights/can_delete_messages")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (
                if can_delete {
                    ChatRole::AdminWithDelete
                } else {
                    ChatRole::AdminLimited
                },
                can_delete,
                true,
            )
        }
        Some("chatMemberStatusMember") => (ChatRole::Member, false, true),
        Some("chatMemberStatusRestricted") => (
            ChatRole::Member,
            false,
            status
                .get("is_member")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        _ => (ChatRole::Member, false, false),
    }
}

fn search_filter(kinds: &[ContentKind]) -> Value {
    if kinds.len() != 1 {
        return Value::Null;
    }
    let filter = match kinds[0] {
        ContentKind::Photo => "searchMessagesFilterPhoto",
        ContentKind::Video => "searchMessagesFilterVideo",
        ContentKind::File => "searchMessagesFilterDocument",
        ContentKind::Voice => "searchMessagesFilterVoiceNote",
        ContentKind::Audio => "searchMessagesFilterAudio",
        ContentKind::Animation => "searchMessagesFilterAnimation",
        _ => return Value::Null,
    };
    json!({ "@type": filter })
}

fn timestamp_i32(value: Option<DateTime<Utc>>) -> i32 {
    value.map_or(0, |value| {
        value.timestamp().clamp(0, i32::MAX as i64) as i32
    })
}

fn display_user_name(user: &Value) -> String {
    let first = user.get("first_name").and_then(Value::as_str).unwrap_or("");
    let last = user.get("last_name").and_then(Value::as_str).unwrap_or("");
    format!("{first} {last}").trim().to_owned()
}

fn bounded_secret<'a>(
    value: &'a str,
    minimum: usize,
    maximum: usize,
    label: &str,
) -> Result<&'a str, AppError> {
    let value = value.trim();
    let count = value.chars().count();
    if count < minimum || count > maximum {
        return Err(AppError::InvalidRequest(format!(
            "{label} length is invalid"
        )));
    }
    Ok(value)
}

fn content_preview(content: &Value) -> (ContentKind, String) {
    let kind = content
        .get("@type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let caption = formatted_text(content.get("caption"));
    let (content_kind, fallback) = match kind {
        "messageText" => (ContentKind::Text, formatted_text(content.get("text"))),
        "messagePhoto" => (ContentKind::Photo, "Photo".into()),
        "messageVideo" | "messageVideoNote" => (ContentKind::Video, "Video".into()),
        "messageDocument" => (
            ContentKind::File,
            content
                .pointer("/document/file_name")
                .and_then(Value::as_str)
                .unwrap_or("File")
                .to_owned(),
        ),
        "messageVoiceNote" => (ContentKind::Voice, "Voice message".into()),
        "messageAudio" => (
            ContentKind::Audio,
            content
                .pointer("/audio/title")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Audio")
                .to_owned(),
        ),
        "messageAnimation" => (ContentKind::Animation, "Animation".into()),
        "messageSticker" => (
            ContentKind::Sticker,
            content
                .pointer("/sticker/emoji")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Sticker")
                .to_owned(),
        ),
        "messagePoll" => (
            ContentKind::Poll,
            formatted_text(content.pointer("/poll/question")),
        ),
        "messageLocation" | "messageVenue" => (ContentKind::Location, "Location".into()),
        "messageContact" => {
            let first = content
                .pointer("/contact/first_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let last = content
                .pointer("/contact/last_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            (
                ContentKind::Contact,
                format!("Contact · {first} {last}").trim().to_owned(),
            )
        }
        value if value.starts_with("message") => {
            (ContentKind::Service, humanize_message_type(value))
        }
        _ => (ContentKind::Other, "Unsupported message".into()),
    };
    let preview = if caption.is_empty() {
        fallback
    } else {
        caption
    };
    (content_kind, preview.chars().take(300).collect())
}

fn sensitive_content_text(content: &Value) -> String {
    let mut parts = Vec::new();
    for path in ["/text/text", "/caption/text", "/poll/question/text"] {
        if let Some(value) = content.pointer(path).and_then(Value::as_str)
            && !value.is_empty()
        {
            parts.push(value.to_owned());
        }
    }
    for path in [
        "/document/file_name",
        "/audio/file_name",
        "/audio/title",
        "/audio/performer",
        "/contact/first_name",
        "/contact/last_name",
        "/contact/phone_number",
        "/venue/title",
        "/venue/address",
    ] {
        if let Some(value) = content.pointer(path).and_then(Value::as_str)
            && !value.is_empty()
        {
            parts.push(value.to_owned());
        }
    }
    let location = content
        .get("location")
        .or_else(|| content.pointer("/venue/location"));
    if let Some(location) = location
        && let (Some(latitude), Some(longitude)) = (
            location.get("latitude").and_then(Value::as_f64),
            location.get("longitude").and_then(Value::as_f64),
        )
    {
        parts.push(format!("{latitude}, {longitude}"));
    }
    if parts.is_empty() {
        parts.push(content_preview(content).1);
    }
    parts.join(" ")
}

fn formatted_text(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn humanize_message_type(value: &str) -> String {
    let value = value.strip_prefix("message").unwrap_or(value);
    let mut output = String::new();
    for character in value.chars() {
        if character.is_uppercase() && !output.is_empty() {
            output.push(' ')
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_text_caption_and_admin_rights() {
        let (kind, preview) = content_preview(&json!({
            "@type": "messagePhoto",
            "caption": { "@type": "formattedText", "text": "Sensitive caption", "entities": [] }
        }));
        assert_eq!(kind, ContentKind::Photo);
        assert_eq!(preview, "Sensitive caption");
        assert_eq!(
            role_from_status(Some(&json!({
                "@type": "chatMemberStatusAdministrator",
                "rights": { "can_delete_messages": true }
            }))),
            (ChatRole::AdminWithDelete, true, true)
        );
    }

    #[test]
    fn parses_int53_and_int64_json_forms() {
        assert_eq!(value_i64(Some(&json!(42))), Some(42));
        assert_eq!(value_i64(Some(&json!("922337"))), Some(922_337));
    }

    #[test]
    fn recognizes_only_visible_main_or_archive_chat_positions() {
        assert!(chat_is_in_catalog(&json!({
            "positions": [{ "list": { "@type": "chatListMain" }, "order": "123" }]
        })));
        assert!(chat_is_in_catalog(&json!({
            "positions": [{ "list": { "@type": "chatListArchive" }, "order": 456 }]
        })));
        assert!(!chat_is_in_catalog(&json!({
            "positions": [{ "list": { "@type": "chatListMain" }, "order": 0 }]
        })));
        assert!(!chat_is_in_catalog(&json!({ "positions": [] })));
    }

    #[test]
    fn refuses_empty_auth_values() {
        assert!(bounded_secret(" ", 1, 32, "code").is_err());
    }

    #[test]
    fn extracts_sensitive_contact_and_location_fields_without_serializing_tdlib_metadata() {
        let contact = json!({
            "@type": "messageContact",
            "contact": {
                "phone_number": "+1 202 555 0147",
                "first_name": "Alex",
                "last_name": "Rivera",
                "user_id": "999999999999"
            }
        });
        let text = sensitive_content_text(&contact);
        assert!(text.contains("+1 202 555 0147"));
        assert!(!text.contains("999999999999"));

        let location = json!({
            "@type": "messageLocation",
            "location": { "latitude": 19.4326, "longitude": -99.1332 }
        });
        assert_eq!(sensitive_content_text(&location), "19.4326, -99.1332");

        let long_text = format!("{} person@example.com", "x".repeat(350));
        let message = json!({
            "@type": "messageText",
            "text": { "@type": "formattedText", "text": long_text, "entities": [] }
        });
        let extracted = sensitive_content_text(&message);
        assert!(extracted.len() > 300);
        assert!(
            detect_sensitive_data(&extracted, ContentKind::Text)
                .contains(&cleaner_domain::SensitiveDataKind::EmailAddress)
        );
    }
}
