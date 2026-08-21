use std::sync::Arc;

use async_trait::async_trait;
use cleaner_domain::{ChatSummary, DeletionReach, MessageSnapshot};

use crate::{
    error::AppError,
    gateway::{GatewayInfo, TelegramGateway},
    model::{AuthSnapshot, AuthStage, CatalogProgress, SearchRequest},
};

pub struct SetupGateway {
    reason: String,
}

impl SetupGateway {
    pub fn new(reason: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            reason: reason.into(),
        })
    }
}

fn setup_error() -> AppError {
    AppError::InvalidRequest("configure Telegram before using Retract".into())
}

#[async_trait]
impl TelegramGateway for SetupGateway {
    fn info(&self) -> GatewayInfo {
        GatewayInfo {
            mode: "live",
            account_label: "Telegram setup required".into(),
            reason: Some(self.reason.clone()),
        }
    }

    fn auth(&self) -> AuthSnapshot {
        AuthSnapshot {
            stage: AuthStage::Error,
            hint: Some(self.reason.clone()),
            qr_link: None,
        }
    }

    fn catalog_progress(&self) -> CatalogProgress {
        CatalogProgress {
            phase: "ready",
            total: 0,
            processed: 0,
        }
    }

    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError> {
        Ok(Vec::new())
    }

    async fn chat_by_id(&self, _chat_id: i64) -> Result<Option<ChatSummary>, AppError> {
        Ok(None)
    }

    async fn search(&self, _request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError> {
        Ok(Vec::new())
    }

    async fn own_messages(&self, _chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        Err(setup_error())
    }

    async fn chat_messages(&self, _chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        Err(setup_error())
    }

    async fn messages_by_ids(&self, _ids: &[(i64, i64)]) -> Result<Vec<MessageSnapshot>, AppError> {
        Err(setup_error())
    }

    async fn sender_name(&self, _sender_id: i64) -> Result<String, AppError> {
        Err(setup_error())
    }

    async fn current_reach(
        &self,
        _chat_id: i64,
        _message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError> {
        Err(setup_error())
    }

    async fn delete_messages_for_everyone(
        &self,
        _chat_id: i64,
        _message_ids: &[i64],
    ) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn clear_history_for_everyone(&self, _chat_id: i64) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn clear_history_for_everyone_keep_chat(&self, _chat_id: i64) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn remove_chat_for_self(&self, _chat_id: i64) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn delete_group(&self, _chat_id: i64) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn leave_chat(&self, _chat_id: i64) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn delete_messages_by_sender(
        &self,
        _chat_id: i64,
        _sender_id: i64,
    ) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn request_qr_auth(&self) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn submit_phone(&self, _phone: &str) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn submit_email_address(&self, _email: &str) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn submit_email_code(&self, _code: &str) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn submit_code(&self, _code: &str) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn submit_password(&self, _password: &str) -> Result<(), AppError> {
        Err(setup_error())
    }

    async fn close(&self) -> Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_gateway_is_inert_and_requires_configuration() {
        tauri::async_runtime::block_on(async {
            let gateway = SetupGateway::new("Telegram setup is incomplete");

            assert_eq!(gateway.info().mode, "live");
            assert!(gateway.chats().await.unwrap().is_empty());
            assert_eq!(gateway.auth().stage, AuthStage::Error);

            let error = gateway.delete_group(7).await.unwrap_err();
            assert!(error.to_string().contains("configure Telegram"));
        });
    }
}
