//! Read-only Telegram queries, independent from cleanup and persistence state.
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt, stream};
use retract_domain::{ActiveContext, ErrorCode, ResourceKind, SafeError, Scope, ScopedResourceRef};
use serde::{Deserialize, Serialize};

use super::{
    diagnostics::{boundary_error, invalid_recipe},
    engine_context::{EngineContext, stale_context},
    locators::{TelegramConversationLocator, TelegramMessageLocator, TelegramPayloadValidator},
    model,
    native::ports::TelegramRead,
    normalize::{normalize_content, normalize_conversation},
};
use crate::{
    compatibility::model_v2 as wire,
    error::AppError,
    persistence::ProviderPayloadValidator,
    provider_service::{safe, validate_refs},
    providers::ports::{
        ApplicationQuery, ContentQuery, ConversationQuery, Page, QuerySource, ResolveRequest,
    },
};

const DIRECT_CHAT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const DIRECT_CHAT_LOOKUP_CONCURRENCY: usize = 8;

pub struct TelegramQuery {
    read: Arc<dyn TelegramRead>,
    context: Arc<EngineContext>,
}

impl TelegramQuery {
    pub fn new(read: Arc<dyn TelegramRead>, context: Arc<EngineContext>) -> Result<Self, AppError> {
        context.check(read.as_ref())?;
        Ok(Self { read, context })
    }

    fn check_scope(&self, scope: &Scope) -> Result<(), AppError> {
        if scope != &self.context.active().scope {
            return Err(stale_context());
        }
        self.context.check(self.read.as_ref())
    }

    fn check_active(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != self.context.active() {
            return Err(safe(ErrorCode::StaleContext));
        }
        self.context
            .check(self.read.as_ref())
            .map_err(boundary_error)
    }

    pub(super) async fn search_native(
        &self,
        scope: &Scope,
        mut request: model::SearchRequest,
    ) -> Result<model::SearchResponse, AppError> {
        self.check_scope(scope)?;
        request.validate()?;
        let requested_limit = request.limit;
        let messages = self.read.search(&request).await;
        self.check_scope(scope)?;
        let messages = messages?;
        let returned = messages.len();
        Ok(model::SearchResponse {
            messages,
            returned,
            truncated: returned == requested_limit,
        })
    }

    async fn lookup_chat_with_timeout(
        &self,
        scope: &Scope,
        chat_id: i64,
    ) -> Result<Option<cleaner_domain::ChatSummary>, AppError> {
        self.check_scope(scope)?;
        let result =
            tokio::time::timeout(DIRECT_CHAT_LOOKUP_TIMEOUT, self.read.chat_by_id(chat_id)).await;
        self.check_scope(scope)?;
        result.map_err(|_| {
            AppError::Timeout(format!(
                "Telegram did not answer the chat refresh within {} seconds. Try again.",
                DIRECT_CHAT_LOOKUP_TIMEOUT.as_secs()
            ))
        })?
    }

    pub(super) async fn refresh_chats(
        &self,
        scope: &Scope,
        mut chat_ids: Vec<i64>,
    ) -> Result<Vec<cleaner_domain::ChatSummary>, AppError> {
        self.check_scope(scope)?;
        if chat_ids.len() > 1_000 || chat_ids.contains(&0) {
            return Err(AppError::InvalidRequest(
                "refresh up to 1,000 valid chats at a time".into(),
            ));
        }
        chat_ids.sort_unstable();
        chat_ids.dedup();
        let chats = stream::iter(chat_ids)
            .map(|chat_id| async move { self.lookup_chat_with_timeout(scope, chat_id).await })
            .buffer_unordered(DIRECT_CHAT_LOOKUP_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await;
        self.check_scope(scope)?;
        let mut chats = chats?.into_iter().flatten().collect::<Vec<_>>();
        chats.sort_by_key(|chat| chat.title.to_lowercase());
        Ok(chats)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramSearchFilters {
    #[serde(default)]
    pub chat_kinds: Vec<cleaner_domain::ChatKind>,
    #[serde(default)]
    pub content_kinds: Vec<cleaner_domain::ContentKind>,
    #[serde(default)]
    pub direction: model::MessageDirection,
    pub min_date: Option<chrono::DateTime<chrono::Utc>>,
    pub max_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub exclude_pinned: bool,
    #[serde(default)]
    pub privacy_scan: bool,
}

fn chat_id(reference: &ScopedResourceRef) -> Result<i64, SafeError> {
    if reference.resource.resource_kind != ResourceKind::Conversation {
        return Err(safe(ErrorCode::ScopeMismatch));
    }
    let locator: TelegramConversationLocator =
        serde_json::from_value(reference.resource.locator_payload.clone())
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
    locator
        .chat_id
        .parse()
        .map_err(|_| safe(ErrorCode::UnsupportedSchema))
}

#[async_trait]
impl ApplicationQuery for TelegramQuery {
    async fn conversations(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<retract_domain::ConversationRecord>, SafeError> {
        self.check_active(context)?;
        let chats = self.read.chats().await;
        self.check_active(context)?;
        let chats = chats.map_err(boundary_error)?;
        chats
            .iter()
            .map(|chat| normalize_conversation(&context.scope, chat).map_err(boundary_error))
            .collect()
    }

    async fn search_filtered(
        &self,
        context: &ActiveContext,
        request: wire::SearchRequest,
    ) -> Result<Page<retract_domain::ContentRecord>, SafeError> {
        self.check_active(context)?;
        validate_refs(
            context,
            &request.conversations,
            &TelegramPayloadValidator,
            Some(ResourceKind::Conversation),
        )?;
        let filters = if let Some(filters) = request.filters {
            if filters.schema != "telegram.search_filters" || filters.version != 1 {
                return Err(safe(ErrorCode::UnsupportedSchema));
            }
            serde_json::from_value::<TelegramSearchFilters>(filters.payload)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
        } else {
            TelegramSearchFilters::default()
        };
        let response = self
            .search_native(
                &context.scope,
                model::SearchRequest {
                    query: request.query,
                    chat_ids: request
                        .conversations
                        .iter()
                        .map(chat_id)
                        .collect::<Result<_, _>>()?,
                    chat_kinds: filters.chat_kinds,
                    content_kinds: filters.content_kinds,
                    direction: filters.direction,
                    min_date: filters.min_date,
                    max_date: filters.max_date,
                    exclude_pinned: filters.exclude_pinned,
                    privacy_scan: filters.privacy_scan,
                    limit: request.limit as usize,
                },
            )
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        Ok(Page {
            items: response
                .messages
                .iter()
                .map(|message| normalize_content(&context.scope, message).map_err(boundary_error))
                .collect::<Result<_, _>>()?,
            next_cursor: None,
        })
    }

    async fn refresh(
        &self,
        context: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<retract_domain::ConversationRecord>, SafeError> {
        self.check_active(context)?;
        validate_refs(
            context,
            &refs,
            &TelegramPayloadValidator,
            Some(ResourceKind::Conversation),
        )?;
        let chats = self
            .refresh_chats(
                &context.scope,
                refs.iter().map(chat_id).collect::<Result<_, _>>()?,
            )
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        chats
            .iter()
            .map(|chat| normalize_conversation(&context.scope, chat).map_err(boundary_error))
            .collect()
    }
}

#[async_trait]
impl QuerySource for TelegramQuery {
    async fn list_conversations(
        &self,
        request: ConversationQuery,
    ) -> Result<Page<retract_domain::ConversationRecord>, retract_domain::ProviderError> {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.cursor.is_some() || request.limit == 0 || request.limit > 100_000 {
            return Err(provider_error(invalid_recipe()));
        }
        let chats = self.read.chats().await;
        self.check_scope(&request.scope).map_err(provider_error)?;
        let chats = chats.map_err(provider_error)?;
        if chats.len() > request.limit as usize {
            return Err(provider_error(invalid_recipe()));
        }
        Ok(Page {
            items: chats
                .iter()
                .map(|chat| normalize_conversation(&request.scope, chat))
                .collect::<Result<_, _>>()
                .map_err(provider_error)?,
            next_cursor: None,
        })
    }

    async fn search(
        &self,
        request: ContentQuery,
    ) -> Result<Page<retract_domain::ContentRecord>, retract_domain::ProviderError> {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.cursor.is_some() {
            return Err(provider_error(invalid_recipe()));
        }
        let response = self
            .search_native(
                &request.scope,
                model::SearchRequest {
                    query: request.query,
                    chat_ids: Vec::new(),
                    chat_kinds: Vec::new(),
                    content_kinds: Vec::new(),
                    direction: model::MessageDirection::Any,
                    min_date: None,
                    max_date: None,
                    exclude_pinned: false,
                    privacy_scan: false,
                    limit: request.limit as usize,
                },
            )
            .await
            .map_err(provider_error)?;
        Ok(Page {
            items: response
                .messages
                .iter()
                .map(|message| normalize_content(&request.scope, message))
                .collect::<Result<_, _>>()
                .map_err(provider_error)?,
            next_cursor: None,
        })
    }

    async fn resolve(
        &self,
        request: ResolveRequest,
    ) -> Result<Vec<retract_domain::ContentRecord>, retract_domain::ProviderError> {
        self.check_scope(&request.scope).map_err(provider_error)?;
        if request.refs.len() > 100_000 {
            return Err(provider_error(invalid_recipe()));
        }
        let mut ids = Vec::new();
        for target in request.refs {
            target
                .validate(&request.scope)
                .map_err(|_| provider_error(invalid_recipe()))?;
            TelegramPayloadValidator
                .validate_resource(&target.resource)
                .map_err(provider_error)?;
            if target.resource.resource_kind != ResourceKind::Content {
                return Err(provider_error(invalid_recipe()));
            }
            let locator: TelegramMessageLocator =
                serde_json::from_value(target.resource.locator_payload)
                    .map_err(|_| provider_error(invalid_recipe()))?;
            ids.push((
                locator
                    .chat_id
                    .parse()
                    .map_err(|_| provider_error(invalid_recipe()))?,
                locator
                    .message_id
                    .parse()
                    .map_err(|_| provider_error(invalid_recipe()))?,
            ));
        }
        let messages = self.read.messages_by_ids(&ids).await;
        self.check_scope(&request.scope).map_err(provider_error)?;
        let messages = messages.map_err(provider_error)?;
        messages
            .iter()
            .map(|message| normalize_content(&request.scope, message).map_err(provider_error))
            .collect()
    }
}

fn provider_error(error: AppError) -> retract_domain::ProviderError {
    retract_domain::ProviderError {
        code: match error {
            AppError::NotFound => retract_domain::ProviderErrorKind::NotFound,
            AppError::Gateway(_) => retract_domain::ProviderErrorKind::PermissionChanged,
            AppError::Timeout(_) => retract_domain::ProviderErrorKind::Transient,
            _ => retract_domain::ProviderErrorKind::AuthenticationRequired,
        },
        retry_at: None,
    }
}
