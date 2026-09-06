//! Telegram connection, identity bootstrap, and authenticated composition.
use crate::providers::telegram::remediation::TelegramCleanup;
use std::sync::Arc;

use async_trait::async_trait;
use retract_domain::{ActiveContext, ErrorCode, SafeError};

use super::{
    LiveGateway,
    diagnostics::boundary_error,
    engine_context::{EngineContext, FoundationTelegramRepository},
    identity::IdentityVerificationStatus,
    model,
    native::ports::{TelegramConnectionIo, TelegramSession},
    query::TelegramQuery,
    registration::TelegramProvider,
};
use crate::{
    compatibility::model_v2 as wire,
    persistence::FoundationStore,
    provider_service::safe,
    providers::ports::{ApplicationConnection, ProviderRegistration},
};

pub struct TelegramConnection {
    gateway: Arc<LiveGateway>,
    store: Arc<FoundationStore>,
}

impl TelegramConnection {
    pub fn new(gateway: Arc<LiveGateway>, store: Arc<FoundationStore>) -> Arc<Self> {
        Arc::new(Self { gateway, store })
    }
}

#[async_trait]
impl ApplicationConnection for TelegramConnection {
    fn context(&self) -> Option<ActiveContext> {
        self.gateway.active_context()
    }

    fn store(&self) -> Option<Arc<FoundationStore>> {
        Some(self.store.clone())
    }

    fn bootstrap(&self) -> Result<wire::BootstrapSnapshot, SafeError> {
        let identity = match self.gateway.identity_verification_status() {
            IdentityVerificationStatus::Unavailable => wire::IdentityStatus::Unavailable,
            IdentityVerificationStatus::Pending => wire::IdentityStatus::Pending,
            IdentityVerificationStatus::Ready => wire::IdentityStatus::Ready,
            IdentityVerificationStatus::Failed { diagnostic } => {
                wire::IdentityStatus::Failed { diagnostic }
            }
        };
        let progress = self.gateway.catalog_progress();
        let mut auth = self.gateway.auth();
        // The legacy auth error may include provider text; v2 exposes predefined copy.
        if matches!(auth.stage, model::AuthStage::Error) {
            auth.hint = Some("Telegram could not continue. Check settings and retry.".into());
        }
        Ok(wire::BootstrapSnapshot {
            identity,
            auth: Some(retract_domain::VersionedPayload {
                schema: "telegram.auth".into(),
                version: 1,
                payload: serde_json::to_value(auth)
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
            }),
            catalog: wire::CatalogProgress {
                phase: match progress.phase {
                    "idle" => wire::CatalogPhase::Idle,
                    "discovering" => wire::CatalogPhase::Discovering,
                    "loading" => wire::CatalogPhase::Loading,
                    "ready" => wire::CatalogPhase::Ready,
                    _ => return Err(safe(ErrorCode::UnsupportedSchema)),
                },
                total: progress.total,
                processed: progress.processed,
            },
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: self
                .store
                .snapshot()
                .map_err(boundary_error)?
                .legacy_history
                .into_iter()
                .map(|record| record.record)
                .collect(),
        })
    }

    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        let active = self
            .context()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        let identity = self
            .gateway
            .verified_identity()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        let context = Arc::new(
            EngineContext::new(active.clone(), identity, self.gateway.session_binding())
                .map_err(boundary_error)?,
        );
        let repository = Arc::new(
            FoundationTelegramRepository::new(self.store.clone(), active.scope)
                .map_err(boundary_error)?,
        );
        let engine = TelegramCleanup::new_scoped(
            self.gateway.clone(),
            self.gateway.clone(),
            context.clone(),
            repository,
        )
        .map_err(boundary_error)?;
        let query = Arc::new(
            TelegramQuery::new(self.gateway.clone(), context.clone()).map_err(boundary_error)?,
        );
        Ok(Arc::new(TelegramProvider::new(query, engine)))
    }

    async fn auth(&self, request: wire::AuthRequest) -> Result<(), SafeError> {
        let value = request.value.as_deref().unwrap_or("");
        match request.operation.as_str() {
            "request_qr_auth" if request.value.is_none() => self.gateway.request_qr_auth().await,
            "submit_phone" => self.gateway.submit_phone(value).await,
            "submit_email_address" => self.gateway.submit_email_address(value).await,
            "submit_email_code" => self.gateway.submit_email_code(value).await,
            "submit_code" => self.gateway.submit_code(value).await,
            "submit_password" => self.gateway.submit_password(value).await,
            _ => return Err(safe(ErrorCode::UnsupportedSchema)),
        }
        .map_err(boundary_error)
    }

    async fn retry_identity(&self) -> Result<(), SafeError> {
        self.gateway.retry_identity_verification()
    }

    async fn shutdown(&self) {
        let _ = self.gateway.close().await;
    }
}
