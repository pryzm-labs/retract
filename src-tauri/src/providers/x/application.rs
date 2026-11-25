use super::{Validator, key};
use crate::{
    compatibility::model_v2 as wire,
    persistence::{
        FoundationStore, ProviderPayloadValidator,
        archive::{ArchiveQuerySource, ArchiveSearch, ArchiveService, ArchiveSourceEntry},
    },
    provider_service::safe,
    providers::{ProviderRegistryError, ports::*},
};
use async_trait::async_trait;
use retract_domain::*;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
struct Workspace {
    ctx: ActiveContext,
    label: String,
    archive: Arc<ArchiveService>,
}
pub(crate) fn build_connection(
    entry: ArchiveSourceEntry,
    archive: Arc<ArchiveService>,
) -> Result<Arc<dyn ApplicationConnection>, SafeError> {
    Validator
        .validate_source(&entry.source, &entry.account)
        .map_err(|_| safe(ErrorCode::InvalidArchive))?;
    Ok(Arc::new(Workspace {
        ctx: ActiveContext {
            scope: entry.source.scope(),
            session_generation: uuid::Uuid::new_v4(),
        },
        label: entry.account.display_name,
        archive,
    }))
}
impl Workspace {
    fn check(&self, ctx: &ActiveContext) -> Result<(), SafeError> {
        if &self.ctx != ctx {
            Err(safe(ErrorCode::StaleContext))
        } else {
            Ok(())
        }
    }
}
#[async_trait]
impl ApplicationConnection for Workspace {
    fn context(&self) -> Option<ActiveContext> {
        Some(self.ctx.clone())
    }
    fn store(&self) -> Option<Arc<FoundationStore>> {
        None
    }
    fn bootstrap(&self) -> Result<wire::BootstrapSnapshot, SafeError> {
        Ok(wire::BootstrapSnapshot {
            identity: wire::IdentityStatus::Ready,
            auth: Some(VersionedPayload {
                schema: "x.account".into(),
                version: 1,
                payload: serde_json::json!({"accountLabel":self.label}),
            }),
            catalog: wire::CatalogProgress {
                phase: wire::CatalogPhase::Ready,
                total: 0,
                processed: 0,
            },
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: vec![],
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        Ok(Arc::new(self.clone()))
    }
    async fn auth(&self, _: wire::AuthRequest) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn shutdown(&self) {}
}
impl ProviderRegistration for Workspace {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            key: key(),
            display_name: "X".into(),
            capabilities: [
                ProviderCapability::ContentSearch,
                ProviderCapability::ConversationListing,
            ]
            .into_iter()
            .collect(),
        }
    }
    fn application_query(&self) -> Result<Arc<dyn ApplicationQuery>, ProviderRegistryError> {
        Ok(Arc::new(self.clone()))
    }
    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        Ok(Arc::new(ArchiveQuerySource(self.archive.clone())))
    }
    fn payload_validator(
        &self,
    ) -> Result<Arc<dyn ProviderPayloadValidator>, ProviderRegistryError> {
        Ok(Arc::new(Validator))
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filters {
    privacy_scan: bool,
}
#[async_trait]
impl ApplicationQuery for Workspace {
    async fn conversations(
        &self,
        ctx: &ActiveContext,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.check(ctx)?;
        Ok(self
            .archive
            .list_conversations(&ConversationQuery {
                scope: ctx.scope.clone(),
                cursor: None,
                limit: 2,
            })
            .await
            .map_err(|_| safe(ErrorCode::InvalidArchive))?
            .items)
    }
    async fn search_filtered(
        &self,
        ctx: &ActiveContext,
        request: wire::SearchRequest,
    ) -> Result<SearchPage<ContentRecord>, SafeError> {
        self.check(ctx)?;
        if request.limit == 0 || request.limit > 10_000 || request.conversations.len() > 1 {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        let filters = match request.filters {
            None => Filters::default(),
            Some(v) if v.schema == "x.search_filters" && v.version == 1 => {
                serde_json::from_value(v.payload).map_err(|_| safe(ErrorCode::UnsupportedSchema))?
            }
            _ => return Err(safe(ErrorCode::UnsupportedSchema)),
        };
        let mut conversations = vec![];
        for item in request.conversations {
            item.validate(&ctx.scope)
                .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
            Validator
                .validate_resource(&item.resource)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
            if item.resource.resource_kind != ResourceKind::Conversation {
                return Err(safe(ErrorCode::UnsupportedSchema));
            }
            conversations.push(
                item.id
                    .try_into()
                    .map_err(|_| safe(ErrorCode::ScopeMismatch))?,
            );
        }
        let mut query = ArchiveSearch {
            scope: ctx.scope.clone(),
            text: request.query,
            kinds: vec![],
            conversations,
            privacy_only: filters.privacy_scan,
            author: None,
            before: None,
            after: None,
            cursor: None,
            limit: 0,
        };
        let mut items = vec![];
        loop {
            query.limit = (request.limit - items.len() as u32).min(200);
            let page = self
                .archive
                .search(&query)
                .await
                .map_err(|_| safe(ErrorCode::InvalidArchive))?;
            items.extend(page.items);
            if page.next_cursor.is_none() || items.len() >= request.limit as usize {
                return Ok(SearchPage {
                    items,
                    completion: if page.next_cursor.is_none() {
                        SearchCompletion::Complete
                    } else {
                        SearchCompletion::ResultLimit
                    },
                    next_cursor: page.next_cursor,
                });
            }
            query.cursor = page.next_cursor;
        }
    }
    async fn refresh(
        &self,
        ctx: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.check(ctx)?;
        self.archive
            .resolve_conversations(&ResolveRequest {
                scope: ctx.scope.clone(),
                refs,
            })
            .await
            .map_err(|_| safe(ErrorCode::InvalidArchive))
    }
}
