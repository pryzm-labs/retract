use async_trait::async_trait;
use retract_domain::{
    ActiveContext, ContentKind, ContentRecord, ConversationRecord, ErrorCode, SafeError,
    ScopedResourceRef, VersionedPayload,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::watch;

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
            ApplicationConnection, ApplicationQuery, ProviderCapability, ProviderDescriptor,
            ProviderRegistration, QuerySource, ReviewedLifecycle, SearchCompletion, SearchPage,
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
    search_generation: watch::Sender<u64>,
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
            search_generation: watch::channel(0).0,
        })
    }

    fn check(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != &self.context {
            Err(safe(ErrorCode::StaleContext))
        } else {
            Ok(())
        }
    }

    fn validate_conversations(&self, refs: &[ScopedResourceRef]) -> Result<(), SafeError> {
        if refs.len() > 1000 {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        for target in refs {
            target
                .validate(&self.context.scope)
                .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
            if target.resource.resource_kind != retract_domain::ResourceKind::Conversation {
                return Err(safe(ErrorCode::UnsupportedSchema));
            }
            DiscordPayloadValidator
                .validate_resource(&target.resource)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        }
        Ok(())
    }
}

// Never represent a superseded request as successful empty/complete results.
fn search_superseded() -> SafeError {
    safe(ErrorCode::SearchCancelled)
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
    ) -> Result<SearchPage<ContentRecord>, SafeError> {
        self.check(context)?;
        let mut changes = self.search_generation.subscribe();
        let mut generation = 0;
        self.search_generation.send_modify(|current| {
            *current = current.wrapping_add(1);
            generation = *current;
        });
        self.validate_conversations(&request.conversations)?;
        if request.limit == 0 || request.limit > 10_000 {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        let filters = match request.filters {
            Some(value) if value.schema == "discord.search_filters" && value.version == 1 => {
                serde_json::from_value(value.payload)
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
            }
            Some(_) => return Err(safe(ErrorCode::UnsupportedSchema)),
            None => DiscordSearchFilters::default(),
        };
        if filters.direction == Direction::Others {
            return Ok(SearchPage {
                items: vec![],
                next_cursor: None,
                completion: SearchCompletion::Complete,
            });
        }
        let selected = request
            .conversations
            .iter()
            .map(|value| {
                value
                    .id
                    .try_into()
                    .map_err(|_| safe(ErrorCode::ScopeMismatch))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cursor = None;
        let mut items = Vec::new();
        loop {
            if *changes.borrow_and_update() != generation {
                return Err(search_superseded());
            }
            let archive_request = ArchiveSearch {
                scope: context.scope.clone(),
                text: request.query.clone(),
                kinds: filters.content_kinds.clone(),
                conversations: selected.clone(),
                privacy_only: filters.privacy_scan,
                author: Some(self.owner_actor),
                before: filters.max_date,
                after: filters.min_date,
                cursor,
                limit: (request.limit - items.len() as u32).min(200),
            };
            // Dropping this read future closes its receiver. The worker can
            // skip a queued read; already executing SQLite work finishes safely.
            let page = {
                let changed = changes.changed();
                let search = self.archives.search(&archive_request);
                futures_util::pin_mut!(changed, search);
                match futures_util::future::select(changed, search).await {
                    futures_util::future::Either::Left(_) => return Err(search_superseded()),
                    futures_util::future::Either::Right((result, _)) => {
                        result.map_err(archive_error)?
                    }
                }
            };
            if *changes.borrow() != generation {
                return Err(search_superseded());
            }
            items.extend(page.items);
            if items.len() >= request.limit as usize || page.next_cursor.is_none() {
                self.check(context)?;
                return Ok(SearchPage {
                    items,
                    completion: if page.next_cursor.is_some() {
                        SearchCompletion::ResultLimit
                    } else {
                        SearchCompletion::Complete
                    },
                    next_cursor: page.next_cursor,
                });
            }
            cursor = page.next_cursor;
        }
    }

    async fn refresh(
        &self,
        context: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.check(context)?;
        self.validate_conversations(&refs)?;
        let refs: Vec<_> = refs
            .into_iter()
            .map(|target| (target.id, target))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();
        let mut records = Vec::new();
        for chunk in refs.chunks(200) {
            records.extend(
                self.archives
                    .resolve_conversations(&crate::providers::ports::ResolveRequest {
                        scope: context.scope.clone(),
                        refs: chunk.to_vec(),
                    })
                    .await
                    .map_err(archive_error)?,
            );
        }
        self.check(context)?;
        records.sort_by_key(|record| record.title.to_lowercase());
        Ok(records)
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
