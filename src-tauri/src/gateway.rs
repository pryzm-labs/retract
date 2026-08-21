use async_trait::async_trait;
use cleaner_domain::{ChatSummary, DeletionReach, MessageSnapshot};

use crate::{
    error::AppError,
    model::{AuthSnapshot, CatalogProgress, SearchRequest},
};

#[derive(Debug, Clone)]
pub struct GatewayInfo {
    pub mode: &'static str,
    pub account_label: String,
    pub reason: Option<String>,
}

#[async_trait]
pub trait TelegramGateway: Send + Sync {
    fn info(&self) -> GatewayInfo;
    fn auth(&self) -> AuthSnapshot;
    fn catalog_progress(&self) -> CatalogProgress;
    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError>;
    async fn chat_by_id(&self, chat_id: i64) -> Result<Option<ChatSummary>, AppError>;
    async fn search(&self, request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError>;
    async fn own_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError>;
    async fn chat_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError>;
    async fn messages_by_ids(&self, ids: &[(i64, i64)]) -> Result<Vec<MessageSnapshot>, AppError>;
    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError>;
    async fn delete_messages_for_everyone(
        &self,
        chat_id: i64,
        message_ids: &[i64],
    ) -> Result<(), AppError>;
    async fn clear_history_for_everyone(&self, chat_id: i64) -> Result<(), AppError>;
    async fn clear_history_for_everyone_keep_chat(&self, chat_id: i64) -> Result<(), AppError>;
    async fn remove_chat_for_self(&self, chat_id: i64) -> Result<(), AppError>;
    async fn delete_group(&self, chat_id: i64) -> Result<(), AppError>;
    async fn leave_chat(&self, chat_id: i64) -> Result<(), AppError>;
    async fn delete_messages_by_sender(&self, chat_id: i64, sender_id: i64)
    -> Result<(), AppError>;
    async fn request_qr_auth(&self) -> Result<(), AppError>;
    async fn submit_phone(&self, phone: &str) -> Result<(), AppError>;
    async fn submit_email_address(&self, email: &str) -> Result<(), AppError>;
    async fn submit_email_code(&self, code: &str) -> Result<(), AppError>;
    async fn submit_code(&self, code: &str) -> Result<(), AppError>;
    async fn submit_password(&self, password: &str) -> Result<(), AppError>;
    async fn close(&self) -> Result<(), AppError>;
}
