//! Test-only provider I/O and connection fixtures. No test implements lifecycle,
//! grants, persistence, recovery, job transitions, or a replacement batch runner.
use super::{commands_v2, model_v2::*, tests::invoke};
use crate::{
    demo_gateway::DemoGateway,
    error::AppError,
    persistence::*,
    provider_service::{ProviderService, safe},
    providers::{
        frozen_lifecycle::{FrozenLifecycle, FrozenProviderIo},
        ports::*,
        registry::ProviderRegistryError,
        telegram::{
            compat::TelegramCompatibilityProvider,
            engine_context::{EngineContext, FoundationTelegramRepository},
            identity::{SessionBinding, TelegramAccountProfile, VerifiedTelegramIdentity},
            locators::*,
            query::TelegramQuery,
            registration::TelegramProvider,
        },
    },
    service::CleanerService,
};
use async_trait::async_trait;
use chrono::Utc;
use retract_domain::*;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use tauri::Manager;

pub const KEY: [u8; 32] = [0x71; 32];
pub fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../src/test/fixtures/provider-lifecycle.json"
    ))
    .unwrap()
}
pub fn context(name: &str) -> ActiveContext {
    serde_json::from_value(fixture()[name].clone()).unwrap()
}
pub fn binding(context: &ActiveContext) -> StoreBinding {
    StoreBinding {
        provider: context.scope.provider.clone(),
        profile: "lifecycle".into(),
    }
}
pub fn open(
    path: &Path,
    context: &ActiveContext,
    validator: Arc<dyn ProviderPayloadValidator>,
) -> Arc<FoundationStore> {
    FoundationStore::open_with_test_key_and_payload_validator(
        path.to_path_buf(),
        binding(context),
        KEY,
        validator,
    )
    .unwrap()
}
pub fn seed(
    store: &FoundationStore,
    context: &ActiveContext,
    native: VersionedPayload,
    source: VersionedPayload,
) {
    store
        .transaction(|state| {
            if !state
                .identities
                .iter()
                .any(|a| a.id == context.scope.account_id)
            {
                state.identities.push(AccountRecord {
                    id: context.scope.account_id,
                    provider: context.scope.provider.clone(),
                    native_identity: native,
                    display_name: "Synthetic account".into(),
                    username: None,
                    avatar: None,
                    connection_state: ConnectionState::Ready,
                    created_at: Utc::now(),
                    last_seen_at: Utc::now(),
                });
                state.sources.push(SourceRecord {
                    id: context.scope.source_id,
                    account_id: context.scope.account_id,
                    provider: context.scope.provider.clone(),
                    kind: SourceKind::LiveConnection,
                    state: SourceState::Ready,
                    archive_fingerprint: None,
                    schema_profile: source,
                    imported_at: None,
                    updated_at: Utc::now(),
                    warnings: vec![],
                });
            }
            Ok(())
        })
        .unwrap();
}
// AAD is a typed ordered JSON struct, so its literal field order is independent
// of the production encoder and must not be alphabetically reordered by Value.
pub fn encrypted_state(
    path: &Path,
    context: &ActiveContext,
    key: [u8; 32],
) -> Result<FoundationState, AppError> {
    let aad = format!(
        "{{\"format\":\"RTRCT03\",\"schema\":3,\"provider\":\"{}\",\"profile\":\"lifecycle\",\"scope\":\"profile\"}}",
        context.scope.provider.as_str()
    );
    let plain = crate::secure_store::decrypt_authenticated(
        &std::fs::read(path.join("jobs.enc"))?,
        b"RTRCT03",
        &key,
        aad.as_bytes(),
    )?;
    serde_json::from_slice(&plain).map_err(|_| AppError::StatePersistenceFailed)
}

pub struct Connection {
    pub active: Arc<RwLock<Option<ActiveContext>>>,
    pub store: Arc<FoundationStore>,
    pub provider: Arc<dyn ProviderRegistration>,
}
#[async_trait]
impl ApplicationConnection for Connection {
    fn context(&self) -> Option<ActiveContext> {
        self.active.read().unwrap().clone()
    }
    fn store(&self) -> Option<Arc<FoundationStore>> {
        Some(self.store.clone())
    }
    fn bootstrap(&self) -> Result<BootstrapSnapshot, SafeError> {
        Ok(BootstrapSnapshot {
            identity: if self.context().is_some() {
                IdentityStatus::Ready
            } else {
                IdentityStatus::Unavailable
            },
            auth: None,
            catalog: CatalogProgress::default(),
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: self
                .store
                .snapshot()
                .unwrap()
                .legacy_history
                .into_iter()
                .map(|r| r.record)
                .collect(),
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        if self.context().is_none() {
            Err(safe(ErrorCode::IdentityUnavailable))
        } else {
            Ok(self.provider.clone())
        }
    }
    async fn auth(&self, _: AuthRequest) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn shutdown(&self) {
        *self.active.write().unwrap() = None;
    }
}
pub struct Harness {
    pub webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    pub service: Arc<ProviderService>,
    pub store: Arc<FoundationStore>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        // The mock runtime retains managed state after closing a window. Replace
        // only its completed connection so reopen tests release the profile lock.
        let runtime = self
            .webview
            .app_handle()
            .state::<Arc<crate::RuntimeState>>();
        *runtime
            .service
            .try_write()
            .expect("fixture has no active command") = ProviderService::setup();
        let _ = self.webview.close();
    }
}
impl Harness {
    pub fn new(connection: Arc<dyn ApplicationConnection>) -> Self {
        let service = ProviderService::new(connection.clone());
        let app = commands_v2::register(tauri::test::mock_builder())
            .manage(Arc::new(crate::RuntimeState::new(service.clone())))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Self {
            webview,
            service,
            store: connection.store().unwrap(),
        }
    }
    pub fn call(
        &self,
        command: &str,
        context: &ActiveContext,
        payload: Value,
    ) -> Result<Value, Value> {
        invoke(
            &self.webview,
            command,
            json!({"contractVersion":2,"context":context,"payload":payload}),
        )
        .map(|r| r["payload"].clone())
    }
    pub fn prepare(&self, context: &ActiveContext, refs: Vec<Value>) -> RemediationPlan {
        serde_json::from_value(
            self.call("prepare_selection_v2", context, json!({"messageRefs":refs}))
                .unwrap(),
        )
        .unwrap()
    }
    pub fn authorize(&self, context: &ActiveContext, plan: &RemediationPlan) {
        self.call(
            "authorize_plan_v2",
            context,
            json!({"planId":plan.id,"fingerprint":plan.fingerprint}),
        )
        .unwrap();
    }
    pub fn start(
        &self,
        context: &ActiveContext,
        plan: &RemediationPlan,
    ) -> Result<ScopedJobRecord, Value> {
        self.call("start_execution_v2",context,json!({"planId":plan.id,"fingerprint":plan.fingerprint,"irreversibleAcknowledged":true,"typedChatTitle":plan.confirmation.exact_text})).map(|v|serde_json::from_value(v).unwrap())
    }
    pub async fn settled(&self, context: &ActiveContext, id: uuid::Uuid) -> ScopedJobRecord {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let jobs: Vec<ScopedJobRecord> =
                    serde_json::from_value(self.call("get_jobs_v2", context, json!({})).unwrap())
                        .unwrap();
                let job = jobs.into_iter().find(|j| j.id == id).unwrap();
                if job.status.is_terminal() || job.status == JobStatus::Blocked {
                    return job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
}

/// Cold-start composition: open the ciphertext without constructing an engine.
/// Identity is published through the real durable SessionBinding; only then may
/// ProviderService request the real scoped Telegram adapter/engine factory.
pub struct PendingTelegram {
    pub store: Arc<FoundationStore>,
    pub gateway: Arc<DemoGateway>,
    native: VerifiedTelegramIdentity,
    binding: Arc<SessionBinding>,
    pub registrations: AtomicUsize,
}
impl PendingTelegram {
    pub async fn reopen(path: &Path, expected: &ActiveContext) -> Arc<Self> {
        let store = open(path, expected, Arc::new(TelegramPayloadValidator));
        let native = VerifiedTelegramIdentity::new(
            TelegramEnvironment::Test,
            42,
            expected.session_generation,
        )
        .unwrap();
        let gateway = Arc::new(DemoGateway::with_verified_identity(native.clone()));
        gateway
            .append_messages(-1001, 9_007_199_254_740_992, 2)
            .await;
        let binding = Arc::new(SessionBinding::default());
        binding
            .begin_generation(expected.session_generation)
            .unwrap();
        Arc::new(Self {
            store,
            gateway,
            native,
            binding,
            registrations: AtomicUsize::new(0),
        })
    }
    pub fn verify(&self) -> ActiveContext {
        self.binding
            .publish(
                &self.store,
                &self.native,
                &TelegramAccountProfile {
                    display_name: "Synthetic account".into(),
                    username: None,
                },
            )
            .unwrap()
    }
}
#[async_trait]
impl ApplicationConnection for PendingTelegram {
    fn context(&self) -> Option<ActiveContext> {
        self.binding.current()
    }
    fn store(&self) -> Option<Arc<FoundationStore>> {
        Some(self.store.clone())
    }
    fn bootstrap(&self) -> Result<BootstrapSnapshot, SafeError> {
        Ok(BootstrapSnapshot {
            identity: if self.context().is_some() {
                IdentityStatus::Ready
            } else {
                IdentityStatus::Pending
            },
            auth: None,
            catalog: CatalogProgress::default(),
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: vec![],
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        let active = self
            .context()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        self.registrations.fetch_add(1, Ordering::AcqRel);
        let context = Arc::new(
            EngineContext::new(active.clone(), self.native.clone(), self.binding.clone()).unwrap(),
        );
        let repository =
            Arc::new(FoundationTelegramRepository::new(self.store.clone(), active.scope).unwrap());
        let engine =
            CleanerService::new_scoped(self.gateway.clone(), context.clone(), repository).unwrap();
        let query = Arc::new(TelegramQuery::new(self.gateway.clone(), context.clone()).unwrap());
        let lifecycle = Arc::new(
            TelegramCompatibilityProvider::new(self.gateway.clone(), context, engine).unwrap(),
        );
        Ok(Arc::new(TelegramProvider::new(query, lifecycle)))
    }
    async fn auth(&self, _: AuthRequest) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        Err(safe(ErrorCode::UnsupportedSchema))
    }
    async fn shutdown(&self) {}
}

pub async fn telegram(path: &Path, active: ActiveContext) -> (Harness, Arc<DemoGateway>) {
    let store = open(path, &active, Arc::new(TelegramPayloadValidator));
    let native = VerifiedTelegramIdentity::new(
        TelegramEnvironment::Test,
        if active.scope.account_id == context("context").scope.account_id {
            42
        } else {
            43
        },
        active.session_generation,
    )
    .unwrap();
    seed(
        &store,
        &active,
        native.account_locator().into_payload(),
        TelegramSourceProfile::payload(),
    );
    let binding = Arc::new(SessionBinding::default());
    binding.begin_generation(active.session_generation).unwrap();
    assert_eq!(
        binding
            .publish(
                &store,
                &native,
                &TelegramAccountProfile {
                    display_name: "Synthetic account".into(),
                    username: None
                }
            )
            .unwrap(),
        active
    );
    let context = Arc::new(EngineContext::new(active.clone(), native.clone(), binding).unwrap());
    let gateway = Arc::new(DemoGateway::with_verified_identity(native));
    gateway
        .append_messages(-1001, 9_007_199_254_740_992, 2)
        .await;
    let repository =
        Arc::new(FoundationTelegramRepository::new(store.clone(), active.scope.clone()).unwrap());
    let engine = CleanerService::new_scoped(gateway.clone(), context.clone(), repository).unwrap();
    let query = Arc::new(TelegramQuery::new(gateway.clone(), context.clone()).unwrap());
    let lifecycle =
        Arc::new(TelegramCompatibilityProvider::new(gateway.clone(), context, engine).unwrap());
    let provider = Arc::new(TelegramProvider::new(query, lifecycle));
    (
        Harness::new(Arc::new(Connection {
            active: Arc::new(RwLock::new(Some(active))),
            store,
            provider,
        })),
        gateway,
    )
}

#[derive(Default)]
pub struct PreflightBarrier {
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}

#[derive(Clone)]
pub struct SyntheticIo {
    pub active: Arc<RwLock<Option<ActiveContext>>>,
    pub calls: Arc<Mutex<Vec<Vec<ScopedResourceRef>>>>,
    pub catalog: Arc<AtomicUsize>,
    pub refreshes: Arc<AtomicUsize>,
    pub invalidate_refresh: Arc<std::sync::atomic::AtomicBool>,
    pub rate_limit_once: Arc<std::sync::atomic::AtomicBool>,
    pub intent_override: Arc<Mutex<Option<Vec<IntentDescriptor>>>>,
    pub rate_deadline: Arc<Mutex<Option<chrono::DateTime<Utc>>>>,
    pub preflights: Arc<Mutex<Vec<chrono::DateTime<Utc>>>>,
    pub preflight_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    pub preflight_barrier: Arc<Mutex<Option<Arc<PreflightBarrier>>>>,
}
impl SyntheticIo {
    pub fn new(active: ActiveContext) -> Self {
        Self {
            active: Arc::new(RwLock::new(Some(active))),
            calls: Arc::new(Mutex::new(vec![])),
            catalog: Arc::new(AtomicUsize::new(0)),
            refreshes: Arc::new(AtomicUsize::new(0)),
            invalidate_refresh: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rate_limit_once: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            intent_override: Arc::new(Mutex::new(None)),
            rate_deadline: Arc::new(Mutex::new(None)),
            preflights: Arc::new(Mutex::new(vec![])),
            preflight_delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            preflight_barrier: Arc::new(Mutex::new(None)),
        }
    }
}
fn descriptor() -> ActionDescriptor {
    ActionDescriptor {
        id: "selected_messages".into(),
        kind: ActionKind::DeleteRemoteItem,
        effect: ExpectedEffect::RemovedForAllParticipants,
        availability: Availability::LivePreflightRequired,
        unavailable_reason: None,
        requires_live_preflight: true,
        batch: BatchConstraints {
            max_targets: 100,
            max_parallel: 1,
        },
        confirmation_tier: ConfirmationTier::Low,
        destructive: true,
        irreversible: true,
        advisory: None,
    }
}
#[async_trait]
impl FrozenProviderIo for SyntheticIo {
    fn check(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if self.active.read().unwrap().as_ref() == Some(context) {
            Ok(())
        } else {
            Err(safe(ErrorCode::StaleContext))
        }
    }
    async fn describe(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
        id: uuid::Uuid,
    ) -> Result<RemediationPlan, SafeError> {
        if intent.action_id != "selected_messages" || intent.actor.is_some() {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        Ok(RemediationPlan {
            id,
            scope: context.scope.clone(),
            steps: vec![ActionStep {
                descriptor: descriptor(),
                targets: intent.targets.clone(),
            }],
            targets: intent.targets,
            confirmation: ConfirmationRequirements {
                tier: ConfirmationTier::Low,
                acknowledgement_required: true,
                owner_auth_required: true,
                exact_text: None,
            },
            recipe: VersionedPayload {
                schema: "synthetic.frozen".into(),
                version: 1,
                payload: json!({"effect":"delete"}),
            },
            restart_policy: RestartPolicy::ResumeFrozenTargets,
            created_at: Utc::now(),
            fingerprint: String::new(),
        })
    }
    fn dirty_refs(&self, _: &RemediationPlan) -> Result<Vec<ScopedResourceRef>, SafeError> {
        Ok(vec![
            serde_json::from_value(fixture()["messages"][2]["conversation"].clone()).unwrap(),
        ])
    }
    async fn owner_prompt(&self, _: &RemediationPlan) -> Result<(), SafeError> {
        Ok(())
    }
    async fn preflight(&self, target: &ScopedResourceRef) -> Result<bool, SafeError> {
        let barrier = self.preflight_barrier.lock().unwrap().take();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
        self.preflights.lock().unwrap().push(Utc::now());
        tokio::time::sleep(std::time::Duration::from_millis(
            self.preflight_delay_ms.load(Ordering::Acquire),
        ))
        .await;
        if self.rate_limit_once.swap(false, Ordering::AcqRel) {
            return Err(SafeError {
                code: ErrorCode::RateLimited,
                retry_at: Some(
                    self.rate_deadline
                        .lock()
                        .unwrap()
                        .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(2)),
                ),
            });
        }
        Ok(target.resource.locator_payload["messageId"] == "message:part/0007")
    }
    async fn mutate(
        &self,
        _: &RemediationPlan,
        targets: &[ScopedResourceRef],
    ) -> Result<(), SafeError> {
        self.calls.lock().unwrap().push(targets.to_vec());
        Ok(())
    }
    async fn intents(
        &self,
        _: &ActiveContext,
        _: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        if let Some(intents) = self.intent_override.lock().unwrap().clone() {
            return Ok(intents);
        }
        Ok(vec![IntentDescriptor {
            action_id: "selected_messages".into(),
            label: "Delete selected".into(),
            requires_actor: false,
            descriptors: vec![descriptor()],
        }])
    }
}
fn synthetic_conversation() -> ConversationRecord {
    let reference: ScopedResourceRef =
        serde_json::from_value(fixture()["messages"][2]["conversation"].clone()).unwrap();
    ConversationRecord {
        id: reference.id.try_into().unwrap(),
        scope: reference.scope,
        resource: reference.resource,
        kind: ConversationKind::Group,
        title: "Synthetic conversation".into(),
        parent_id: None,
        participant_count: None,
        participants: vec![],
        evidence: EvidenceState::Live,
        observed_at: Utc::now(),
        provider_metadata: None,
    }
}
#[async_trait]
impl ApplicationQuery for SyntheticIo {
    async fn conversations(&self, _: &ActiveContext) -> Result<Vec<ConversationRecord>, SafeError> {
        self.catalog.fetch_add(1, Ordering::SeqCst);
        Ok(vec![synthetic_conversation()])
    }
    async fn search_filtered(
        &self,
        _: &ActiveContext,
        _: SearchRequest,
    ) -> Result<Page<ContentRecord>, SafeError> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }
    async fn refresh(
        &self,
        _: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<ConversationRecord>, SafeError> {
        self.refreshes.fetch_add(refs.len(), Ordering::SeqCst);
        tokio::task::yield_now().await;
        if self.invalidate_refresh.load(Ordering::Acquire) {
            self.active
                .write()
                .unwrap()
                .as_mut()
                .unwrap()
                .session_generation = uuid::Uuid::new_v4();
        }
        Ok(vec![synthetic_conversation()])
    }
}
#[async_trait]
impl QuerySource for SyntheticIo {
    async fn list_conversations(
        &self,
        request: ConversationQuery,
    ) -> Result<Page<ConversationRecord>, ProviderError> {
        let _ = request;
        Ok(Page {
            items: vec![synthetic_conversation()],
            next_cursor: None,
        })
    }
    async fn search(&self, _: ContentQuery) -> Result<Page<ContentRecord>, ProviderError> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }
    async fn resolve(&self, _: ResolveRequest) -> Result<Vec<ContentRecord>, ProviderError> {
        Ok(vec![])
    }
}
#[derive(Clone)]
struct SyntheticRegistration {
    io: Arc<SyntheticIo>,
    lifecycle: FrozenLifecycle,
}
impl ProviderRegistration for SyntheticRegistration {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            key: context("syntheticContext").scope.provider,
            display_name: "Synthetic test I/O".into(),
            capabilities: [ProviderCapability::ContentSearch].into_iter().collect(),
        }
    }
    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        Ok(self.io.clone())
    }
    fn application_query(&self) -> Result<Arc<dyn ApplicationQuery>, ProviderRegistryError> {
        Ok(self.io.clone())
    }
    fn reviewed_lifecycle(&self) -> Result<Arc<dyn ReviewedLifecycle>, ProviderRegistryError> {
        Ok(Arc::new(self.lifecycle.clone()))
    }
    fn payload_validator(
        &self,
    ) -> Result<Arc<dyn ProviderPayloadValidator>, ProviderRegistryError> {
        Ok(Arc::new(SyntheticValidator))
    }
}
pub struct SyntheticValidator;
impl ProviderPayloadValidator for SyntheticValidator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "synthetic-lifecycle-fixture-v1"
            .to_string()
            .try_into()
            .unwrap()
    }
    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        if account.native_identity.schema != "synthetic.account"
            || account.native_identity.version != 1
            || account.native_identity.payload != json!({"id":"account:fixture"})
        {
            return Err(AppError::StatePersistenceFailed);
        }
        "synthetic:account:fixture".to_string().try_into()
    }
    fn validate_source(&self, source: &SourceRecord, _: &AccountRecord) -> Result<(), AppError> {
        if source.schema_profile.schema == "synthetic.source"
            && source.schema_profile.version == 1
            && source.schema_profile.payload == json!({})
        {
            Ok(())
        } else {
            Err(AppError::StatePersistenceFailed)
        }
    }
    fn validate_resource(&self, r: &ProviderResourceRef) -> Result<(), AppError> {
        let p = &r.locator_payload;
        let expected = match r.resource_kind {
            ResourceKind::Conversation
                if r.locator_schema == "synthetic.conversation"
                    && p.as_object().is_some_and(|o| o.len() == 1)
                    && p["chatId"].is_string() =>
            {
                serde_json::to_string(&vec![p["chatId"].as_str().unwrap()]).unwrap()
            }
            ResourceKind::Content
                if r.locator_schema == "synthetic.message"
                    && p.as_object().is_some_and(|o| o.len() == 2)
                    && p["chatId"].is_string()
                    && p["messageId"].is_string() =>
            {
                serde_json::to_string(&vec![
                    p["chatId"].as_str().unwrap(),
                    p["messageId"].as_str().unwrap(),
                ])
                .unwrap()
            }
            _ => return Err(AppError::StatePersistenceFailed),
        };
        if r.locator_version == 1 && r.canonical_key == expected {
            Ok(())
        } else {
            Err(AppError::StatePersistenceFailed)
        }
    }
    fn validate_recipe(&self, plan: &RemediationPlan) -> Result<(), AppError> {
        if plan.recipe.schema == "synthetic.frozen"
            && plan.recipe.version == 1
            && plan.recipe.payload == json!({"effect":"delete"})
            && plan.steps.iter().all(|s| s.descriptor == descriptor())
        {
            Ok(())
        } else {
            Err(AppError::StatePersistenceFailed)
        }
    }
    fn validate_job(&self, plan: &RemediationPlan, job: &ScopedJobRecord) -> Result<(), AppError> {
        if job.next_batch > 1
            || job.counters.selected != plan.targets.len() as u64
            || job.dirty_refs
                != vec![
                    serde_json::from_value::<ScopedResourceRef>(
                        fixture()["messages"][2]["conversation"].clone(),
                    )
                    .unwrap(),
                ]
        {
            Err(AppError::StatePersistenceFailed)
        } else {
            Ok(())
        }
    }
}
pub fn synthetic(path: &Path, io: Arc<SyntheticIo>) -> Harness {
    let context = context("syntheticContext");
    let store = open(path, &context, Arc::new(SyntheticValidator));
    seed(
        &store,
        &context,
        VersionedPayload {
            schema: "synthetic.account".into(),
            version: 1,
            payload: json!({"id":"account:fixture"}),
        },
        VersionedPayload {
            schema: "synthetic.source".into(),
            version: 1,
            payload: json!({}),
        },
    );
    let lifecycle = FrozenLifecycle::new(context.clone(), io.clone(), store.clone()).unwrap();
    Harness::new(Arc::new(Connection {
        active: io.active.clone(),
        store,
        provider: Arc::new(SyntheticRegistration { io, lifecycle }),
    }))
}
