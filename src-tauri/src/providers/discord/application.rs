use async_trait::async_trait;
use retract_domain::{
    ActiveContext, ContentKind, ContentRecord, ConversationRecord, ErrorCode, SafeError,
    ScopedResourceRef, VersionedPayload,
};
use serde::Deserialize;
use std::{collections::HashSet, sync::Arc};

use super::{
    DiscordPayloadValidator,
    locators::{DiscordUserLocator, discord_provider_key},
    remediation::DiscordRemediationIo,
    session::DiscordSessionOwner,
};
use crate::{
    compatibility::model_v2::{self as wire, BootstrapSnapshot},
    persistence::{
        FoundationStore, ProviderPayloadValidator,
        archive::{ArchiveQuerySource, ArchiveSearch, ArchiveService, ArchiveSourceEntry},
    },
    provider_service::safe,
    providers::{
        frozen_lifecycle::FrozenLifecycle,
        ports::{
            ApplicationConnection, ApplicationQuery, Page, ProviderCapability, ProviderDescriptor,
            ProviderRegistration, QuerySource, ReviewedLifecycle,
        },
        registry::ProviderRegistryError,
    },
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscordSearchFilters {
    #[serde(default)]
    content_kinds: Vec<ContentKind>,
    #[serde(default)]
    direction: Direction,
    min_date: Option<chrono::DateTime<chrono::Utc>>,
    max_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    privacy_scan: bool,
}

#[derive(Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Direction {
    #[default]
    Any,
    Mine,
    Others,
}

pub(crate) struct DiscordApplicationQuery {
    context: ActiveContext,
    owner_actor: retract_domain::ActorId,
    archives: Arc<ArchiveService>,
}

impl DiscordApplicationQuery {
    pub(crate) fn new(
        context: ActiveContext,
        owner_user_id: &str,
        archives: Arc<ArchiveService>,
    ) -> Result<Self, SafeError> {
        let resource = DiscordUserLocator::new(owner_user_id)
            .map_err(|_| safe(ErrorCode::InvalidArchive))?
            .resource(context.scope.account_id);
        let owner_actor = resource
            .resource_id()
            .ok()
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| safe(ErrorCode::InvalidArchive))?;
        Ok(Self {
            context,
            owner_actor,
            archives,
        })
    }

    fn check(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != &self.context {
            Err(safe(ErrorCode::StaleContext))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl ApplicationQuery for DiscordApplicationQuery {
    async fn conversations(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.check(context)?;
        let mut cursor = None;
        let mut records = Vec::new();
        loop {
            let page = self
                .archives
                .list_conversations(&crate::providers::ports::ConversationQuery {
                    scope: context.scope.clone(),
                    cursor,
                    limit: 200,
                })
                .await
                .map_err(archive_error)?;
            records.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
            if records.len() > 100_000 {
                return Err(safe(ErrorCode::InvalidArchive));
            }
        }
        self.check(context)?;
        records.sort_by_key(|record| record.title.to_lowercase());
        Ok(records)
    }

    async fn search_filtered(
        &self,
        context: &ActiveContext,
        request: wire::SearchRequest,
    ) -> Result<Page<ContentRecord>, SafeError> {
        self.check(context)?;
        let filters = match request.filters {
            Some(value) if value.schema == "discord.search_filters" && value.version == 1 => {
                serde_json::from_value(value.payload)
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
            }
            Some(_) => return Err(safe(ErrorCode::UnsupportedSchema)),
            None => DiscordSearchFilters::default(),
        };
        if filters.direction == Direction::Others {
            return Ok(Page {
                items: vec![],
                next_cursor: None,
            });
        }
        let selected: HashSet<_> = request.conversations.iter().map(|value| value.id).collect();
        let mut cursor = None;
        let mut items = Vec::new();
        loop {
            let page = self
                .archives
                .search(&ArchiveSearch {
                    scope: context.scope.clone(),
                    text: request.query.clone(),
                    kinds: filters.content_kinds.clone(),
                    author: Some(self.owner_actor),
                    before: filters.max_date,
                    after: filters.min_date,
                    cursor,
                    limit: 200,
                })
                .await
                .map_err(archive_error)?;
            items.extend(page.items.into_iter().filter(|item| {
                (selected.is_empty() || selected.contains(item.conversation_id.as_uuid()))
                    && (!filters.privacy_scan || !item.privacy_findings.is_empty())
            }));
            if items.len() >= request.limit as usize || page.next_cursor.is_none() {
                break;
            }
            cursor = page.next_cursor;
        }
        items.truncate(request.limit as usize);
        self.check(context)?;
        Ok(Page {
            items,
            next_cursor: None,
        })
    }

    async fn refresh(
        &self,
        context: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.check(context)?;
        let wanted: HashSet<_> = refs.iter().map(|value| value.id).collect();
        let all = self.conversations(context).await?;
        Ok(all
            .into_iter()
            .filter(|item| wanted.contains(item.id.as_uuid()))
            .collect())
    }
}

pub(crate) struct DiscordProvider {
    query: Arc<DiscordApplicationQuery>,
    lifecycle: Arc<FrozenLifecycle>,
}

impl DiscordProvider {
    pub(crate) fn new(query: Arc<DiscordApplicationQuery>, lifecycle: FrozenLifecycle) -> Self {
        Self {
            query,
            lifecycle: Arc::new(lifecycle),
        }
    }
}

fn discord_descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        key: discord_provider_key(),
        display_name: "Discord".into(),
        capabilities: [
            ProviderCapability::ConversationListing,
            ProviderCapability::ContentSearch,
            ProviderCapability::MediaMetadata,
        ]
        .into_iter()
        .collect(),
    }
}

impl ProviderRegistration for DiscordProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        discord_descriptor()
    }
    fn reviewed_lifecycle(&self) -> Result<Arc<dyn ReviewedLifecycle>, ProviderRegistryError> {
        Ok(self.lifecycle.clone())
    }
    fn application_query(&self) -> Result<Arc<dyn ApplicationQuery>, ProviderRegistryError> {
        Ok(self.query.clone())
    }
    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        Ok(Arc::new(ArchiveQuerySource(self.query.archives.clone())))
    }
    fn payload_validator(
        &self,
    ) -> Result<Arc<dyn ProviderPayloadValidator>, ProviderRegistryError> {
        Ok(Arc::new(DiscordPayloadValidator))
    }
}

pub(crate) struct DiscordConnection {
    context: ActiveContext,
    account_label: String,
    store: Arc<FoundationStore>,
    provider: Arc<DiscordProvider>,
}

impl DiscordConnection {
    pub(crate) fn new(
        context: ActiveContext,
        account_label: String,
        store: Arc<FoundationStore>,
        provider: DiscordProvider,
    ) -> Arc<Self> {
        Arc::new(Self {
            context,
            account_label,
            store,
            provider: Arc::new(provider),
        })
    }
}

#[async_trait]
impl ApplicationConnection for DiscordConnection {
    fn context(&self) -> Option<ActiveContext> {
        Some(self.context.clone())
    }
    fn store(&self) -> Option<Arc<FoundationStore>> {
        Some(self.store.clone())
    }
    fn bootstrap(&self) -> Result<BootstrapSnapshot, SafeError> {
        Ok(BootstrapSnapshot {
            identity: wire::IdentityStatus::Ready,
            auth: Some(VersionedPayload {
                schema: "discord.account".to_owned(),
                version: 1,
                payload: serde_json::json!({ "accountLabel": self.account_label }),
            }),
            catalog: wire::CatalogProgress {
                phase: wire::CatalogPhase::Ready,
                total: 0,
                processed: 0,
            },
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: self
                .store
                .snapshot()
                .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?
                .legacy_history
                .into_iter()
                .map(|item| item.record)
                .collect(),
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        Ok(self.provider.clone())
    }
    async fn auth(&self, _request: wire::AuthRequest) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        Ok(())
    }
    async fn shutdown(&self) {}
}

pub(crate) fn build_connection(
    root: &std::path::Path,
    entry: ArchiveSourceEntry,
    archives: Arc<ArchiveService>,
    session: Arc<DiscordSessionOwner>,
) -> Result<Arc<dyn ApplicationConnection>, SafeError> {
    let locator: DiscordUserLocator =
        serde_json::from_value(entry.account.native_identity.payload.clone())
            .map_err(|_| safe(ErrorCode::InvalidArchive))?;
    let context = ActiveContext {
        scope: entry.source.scope(),
        session_generation: uuid::Uuid::new_v4(),
    };
    let profile = format!("discord-{}", entry.account.id.as_uuid());
    let store = FoundationStore::open_with_payload_validator(
        root.join(&profile),
        crate::persistence::StoreBinding {
            provider: discord_provider_key(),
            profile,
        },
        Arc::new(DiscordPayloadValidator),
    )
    .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?;
    store
        .transaction(|state| {
            if !state
                .identities
                .iter()
                .any(|item| item.id == entry.account.id)
            {
                state.identities.push(entry.account.clone());
            }
            if !state.sources.iter().any(|item| item.id == entry.source.id) {
                state.sources.push(entry.source.clone());
            }
            Ok(())
        })
        .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?;
    let query = Arc::new(DiscordApplicationQuery::new(
        context.clone(),
        &locator.user_id,
        archives.clone(),
    )?);
    let io = Arc::new(DiscordRemediationIo::production(
        context.clone(),
        locator.user_id,
        Arc::new(ArchiveQuerySource(archives)),
        session,
    )?);
    let lifecycle = FrozenLifecycle::new(context.clone(), io, store.clone())?;
    Ok(DiscordConnection::new(
        context,
        entry.account.display_name,
        store,
        DiscordProvider::new(query, lifecycle),
    ))
}

fn archive_error(error: crate::persistence::archive::ArchiveError) -> SafeError {
    safe(match error {
        crate::persistence::archive::ArchiveError::ScopeMismatch => ErrorCode::NotFound,
        crate::persistence::archive::ArchiveError::UnsupportedSchema => {
            ErrorCode::UnsupportedSchema
        }
        crate::persistence::archive::ArchiveError::InvalidRecord
        | crate::persistence::archive::ArchiveError::InvalidStore => ErrorCode::InvalidArchive,
        crate::persistence::archive::ArchiveError::UnavailableKey => {
            ErrorCode::AuthenticationRequired
        }
        _ => ErrorCode::Transient,
    })
}

#[cfg(test)]
mod registration_tests {
    use super::*;

    #[test]
    fn selected_archive_declares_only_capabilities_backed_by_registration_ports() {
        let descriptor = discord_descriptor();
        assert_eq!(
            descriptor.capabilities,
            [
                ProviderCapability::ConversationListing,
                ProviderCapability::ContentSearch,
                ProviderCapability::MediaMetadata,
            ]
            .into_iter()
            .collect()
        );
    }
}
