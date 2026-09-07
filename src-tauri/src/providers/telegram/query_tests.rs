use crate::providers::telegram::remediation::TelegramCleanup;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use cleaner_domain::{ChatSummary, DeletionReach, MessageSnapshot};
use retract_domain::{ErrorCode, ProviderErrorKind};

use crate::{
    compatibility::model_v2 as wire,
    demo_gateway::DemoGateway,
    error::AppError,
    persistence::{FoundationStore, StoreBinding},
    providers::ports::{
        ApplicationQuery, ContentQuery, ConversationQuery, ProviderCapability,
        ProviderRegistration, QuerySource, ResolveRequest,
    },
};

use super::{
    engine_context::{EngineContext, FoundationTelegramRepository},
    identity::{SessionBinding, TelegramAccountProfile, VerifiedTelegramIdentity},
    locators::{TelegramEnvironment, TelegramPayloadValidator, telegram_provider_key},
    model::{AuthSnapshot, CatalogProgress, MessageDirection, SearchRequest},
    native::ports::{GatewayInfo, TelegramRead, TelegramSession},
    normalize::{conversation_ref, normalize_content},
    query::TelegramQuery,
    registration::TelegramProvider,
};

struct ReadOnlyFake {
    inner: Arc<DemoGateway>,
    identity: RwLock<Option<VerifiedTelegramIdentity>>,
    invalidate_search: AtomicBool,
    invalidate_resolve: AtomicBool,
}

impl ReadOnlyFake {
    fn new(identity: VerifiedTelegramIdentity) -> Self {
        Self {
            inner: Arc::new(DemoGateway::with_verified_identity(identity.clone())),
            identity: RwLock::new(Some(identity)),
            invalidate_search: AtomicBool::new(false),
            invalidate_resolve: AtomicBool::new(false),
        }
    }

    fn invalidate(&self) {
        *self.identity.write().unwrap() = None;
    }
}

impl TelegramSession for ReadOnlyFake {
    fn info(&self) -> GatewayInfo {
        self.inner.info()
    }

    fn auth(&self) -> AuthSnapshot {
        self.inner.auth()
    }

    fn verified_identity(&self) -> Option<VerifiedTelegramIdentity> {
        self.identity.read().unwrap().clone()
    }

    fn catalog_progress(&self) -> CatalogProgress {
        self.inner.catalog_progress()
    }
}

#[async_trait::async_trait]
impl TelegramRead for ReadOnlyFake {
    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError> {
        self.inner.chats().await
    }

    async fn chat_by_id(&self, chat_id: i64) -> Result<Option<ChatSummary>, AppError> {
        self.inner.chat_by_id(chat_id).await
    }

    async fn search(&self, request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError> {
        let result = self.inner.search(request).await;
        if self.invalidate_search.load(Ordering::Acquire) {
            self.invalidate();
        }
        result
    }

    async fn own_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        self.inner.own_messages(chat_id).await
    }

    async fn chat_messages(&self, chat_id: i64) -> Result<Vec<MessageSnapshot>, AppError> {
        self.inner.chat_messages(chat_id).await
    }

    async fn messages_by_ids(&self, ids: &[(i64, i64)]) -> Result<Vec<MessageSnapshot>, AppError> {
        let result = self.inner.messages_by_ids(ids).await;
        if self.invalidate_resolve.load(Ordering::Acquire) {
            self.invalidate();
        }
        result
    }

    async fn sender_name(&self, sender_id: i64) -> Result<String, AppError> {
        self.inner.sender_name(sender_id).await
    }

    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, AppError> {
        self.inner.current_reach(chat_id, message_id).await
    }
}

fn read_only_fixture() -> (
    tempfile::TempDir,
    retract_domain::ActiveContext,
    Arc<ReadOnlyFake>,
    TelegramQuery,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = FoundationStore::open_with_test_key_and_payload_validator(
        directory.path().join("profile"),
        StoreBinding {
            provider: telegram_provider_key(),
            profile: "read-only-query".into(),
        },
        [0x74; 32],
        Arc::new(TelegramPayloadValidator),
    )
    .unwrap();
    let identity =
        VerifiedTelegramIdentity::new(TelegramEnvironment::Test, 42, uuid::Uuid::new_v4()).unwrap();
    let binding = Arc::new(SessionBinding::default());
    binding
        .begin_generation(identity.session_generation)
        .unwrap();
    let active = binding
        .publish(
            &store,
            &identity,
            &TelegramAccountProfile {
                display_name: "Synthetic account".into(),
                username: None,
            },
        )
        .unwrap();
    let context = Arc::new(EngineContext::new(active.clone(), identity.clone(), binding).unwrap());
    let fake = Arc::new(ReadOnlyFake::new(identity));
    let read: Arc<dyn TelegramRead> = fake.clone();
    let query = TelegramQuery::new(read, context).unwrap();
    (directory, active, fake, query)
}

#[test]
fn retained_query_does_not_keep_cleanup_owner_alive() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = FoundationStore::open_with_test_key_and_payload_validator(
            directory.path().join("profile"),
            StoreBinding {
                provider: telegram_provider_key(),
                profile: "query-ownership".into(),
            },
            [0x73; 32],
            Arc::new(TelegramPayloadValidator),
        )
        .unwrap();
        let identity =
            VerifiedTelegramIdentity::new(TelegramEnvironment::Test, 42, uuid::Uuid::new_v4())
                .unwrap();
        let binding = Arc::new(SessionBinding::default());
        binding
            .begin_generation(identity.session_generation)
            .unwrap();
        let active = binding
            .publish(
                &store,
                &identity,
                &TelegramAccountProfile {
                    display_name: "Synthetic account".into(),
                    username: None,
                },
            )
            .unwrap();
        let context = Arc::new(
            EngineContext::new(active.clone(), identity.clone(), binding.clone()).unwrap(),
        );
        let gateway = Arc::new(DemoGateway::with_verified_identity(identity));
        let repository =
            Arc::new(FoundationTelegramRepository::new(store, active.scope.clone()).unwrap());
        let engine = TelegramCleanup::new_scoped(
            gateway.clone(),
            gateway.clone(),
            context.clone(),
            repository,
        )
        .unwrap();
        let query = Arc::new(TelegramQuery::new(gateway.clone(), context.clone()).unwrap());
        let lifecycle = engine.clone();
        let registration: Arc<dyn ProviderRegistration> =
            Arc::new(TelegramProvider::new(query, lifecycle));
        assert_eq!(
            registration.descriptor().capabilities,
            [
                ProviderCapability::ConversationListing,
                ProviderCapability::ContentSearch,
                ProviderCapability::MediaMetadata,
            ]
            .into_iter()
            .collect()
        );
        assert!(registration.remediation().is_err());
        assert!(registration.live_connection().is_err());

        let weak_cleanup = Arc::downgrade(&engine);
        let query = registration.application_query().unwrap();
        drop(registration);
        drop(engine);
        assert!(
            weak_cleanup.upgrade().is_none(),
            "query must not retain cleanup state"
        );

        let page = query
            .search_filtered(
                &active,
                wire::SearchRequest {
                    query: "Passport scan".into(),
                    conversations: Vec::new(),
                    filters: None,
                    limit: 100,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].searchable_text,
            "Passport scan for the apartment application"
        );
    });
}

#[test]
fn read_only_query_refresh_deduplicates_sorts_omits_missing_and_never_reads_catalog() {
    tauri::async_runtime::block_on(async {
        let (_directory, active, fake, query) = read_only_fixture();
        let refs = vec![
            conversation_ref(&active.scope, -1001).unwrap(),
            conversation_ref(&active.scope, -1001).unwrap(),
            conversation_ref(&active.scope, -999).unwrap(),
            conversation_ref(&active.scope, 304).unwrap(),
        ];

        let chats = query.refresh(&active, refs).await.unwrap();

        assert_eq!(
            chats
                .iter()
                .map(|chat| chat.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Design Team", "Empty invite"]
        );
        assert_eq!(fake.inner.chat_read_counts(), (0, 3));
    });
}

#[test]
fn read_only_query_rejects_zero_and_oversized_refresh_without_native_reads() {
    tauri::async_runtime::block_on(async {
        let (_directory, active, fake, query) = read_only_fixture();

        assert!(query.refresh_chats(&active.scope, vec![0]).await.is_err());
        assert!(
            query
                .refresh_chats(&active.scope, vec![101; 1_001])
                .await
                .is_err()
        );
        assert_eq!(fake.inner.chat_read_counts(), (0, 0));
    });
}

#[test]
fn native_search_validates_limits_and_marks_a_full_page_conservatively_truncated() {
    tauri::async_runtime::block_on(async {
        let (_directory, active, _fake, query) = read_only_fixture();
        let request = |limit| SearchRequest {
            query: String::new(),
            chat_ids: Vec::new(),
            chat_kinds: Vec::new(),
            content_kinds: Vec::new(),
            direction: MessageDirection::Any,
            min_date: None,
            max_date: None,
            exclude_pinned: false,
            privacy_scan: false,
            limit,
        };

        assert!(
            query
                .search_native(&active.scope, request(0))
                .await
                .is_err()
        );
        assert!(
            query
                .search_native(&active.scope, request(10_001))
                .await
                .is_err()
        );
        let response = query
            .search_native(&active.scope, request(1))
            .await
            .unwrap();
        assert_eq!(response.returned, 1);
        assert_eq!(response.messages.len(), 1);
        assert!(response.truncated);
    });
}

#[test]
fn query_source_keeps_cursor_and_conversation_page_contracts_unsupported() {
    tauri::async_runtime::block_on(async {
        let (_directory, active, _fake, query) = read_only_fixture();

        let list_error = query
            .list_conversations(ConversationQuery {
                scope: active.scope.clone(),
                cursor: Some("opaque".into()),
                limit: 100,
            })
            .await
            .unwrap_err();
        assert_eq!(list_error.code, ProviderErrorKind::AuthenticationRequired);
        let search_error = query
            .search(ContentQuery {
                scope: active.scope.clone(),
                query: String::new(),
                cursor: Some("opaque".into()),
                limit: 100,
            })
            .await
            .unwrap_err();
        assert_eq!(search_error.code, ProviderErrorKind::AuthenticationRequired);
        let page_error = query
            .list_conversations(ConversationQuery {
                scope: active.scope,
                cursor: None,
                limit: 1,
            })
            .await
            .unwrap_err();
        assert_eq!(page_error.code, ProviderErrorKind::AuthenticationRequired);
    });
}

#[test]
fn query_and_resolve_discard_results_when_identity_changes_during_await() {
    tauri::async_runtime::block_on(async {
        let (_directory, active, fake, query) = read_only_fixture();
        fake.invalidate_search.store(true, Ordering::Release);
        let error = query
            .search_filtered(
                &active,
                wire::SearchRequest {
                    query: "Passport".into(),
                    conversations: Vec::new(),
                    filters: None,
                    limit: 100,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleContext);

        let (_directory, active, fake, query) = read_only_fixture();
        fake.invalidate_resolve.store(true, Ordering::Release);
        let message = fake
            .inner
            .search(&SearchRequest {
                query: "Passport".into(),
                chat_ids: Vec::new(),
                chat_kinds: Vec::new(),
                content_kinds: Vec::new(),
                direction: MessageDirection::Any,
                min_date: None,
                max_date: None,
                exclude_pinned: false,
                privacy_scan: false,
                limit: 100,
            })
            .await
            .unwrap()
            .remove(0);
        let content = normalize_content(&active.scope, &message).unwrap();
        let reference = retract_domain::ScopedResourceRef {
            scope: active.scope.clone(),
            id: *content.id.as_uuid(),
            resource: content.resource,
        };
        let error = query
            .resolve(ResolveRequest {
                scope: active.scope,
                refs: vec![reference],
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ProviderErrorKind::AuthenticationRequired);
    });
}
