//! Registration exposes independent query and reviewed-cleanup handles.
use std::sync::Arc;

use super::{
    locators::{TelegramPayloadValidator, telegram_provider_key},
    query::TelegramQuery,
    remediation::TelegramCleanup,
};
use crate::{
    persistence::ProviderPayloadValidator,
    providers::{ports::*, registry::ProviderRegistryError},
};

pub struct TelegramProvider {
    query: Arc<TelegramQuery>,
    reviewed_lifecycle: Arc<TelegramCleanup>,
}

impl TelegramProvider {
    pub fn new(query: Arc<TelegramQuery>, reviewed_lifecycle: Arc<TelegramCleanup>) -> Self {
        Self {
            query,
            reviewed_lifecycle,
        }
    }
}

impl ProviderRegistration for TelegramProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            key: telegram_provider_key(),
            display_name: "Telegram".into(),
            capabilities: [
                ProviderCapability::ConversationListing,
                ProviderCapability::ContentSearch,
                ProviderCapability::MediaMetadata,
            ]
            .into_iter()
            .collect(),
        }
    }

    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        Ok(self.query.clone())
    }

    fn application_query(&self) -> Result<Arc<dyn ApplicationQuery>, ProviderRegistryError> {
        Ok(self.query.clone())
    }

    fn reviewed_lifecycle(&self) -> Result<Arc<dyn ReviewedLifecycle>, ProviderRegistryError> {
        Ok(self.reviewed_lifecycle.clone())
    }

    fn payload_validator(
        &self,
    ) -> Result<Arc<dyn ProviderPayloadValidator>, ProviderRegistryError> {
        Ok(Arc::new(TelegramPayloadValidator))
    }
}
