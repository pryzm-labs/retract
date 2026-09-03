//! Version, identity and resource validation for every production application call.
use crate::{
    compatibility::model_v2::*,
    providers::{ports::*, registry::ProviderRegistry},
};
use retract_domain::{ActiveContext, ErrorCode, ResourceKind, SafeError, ScopedResourceRef};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex;

pub fn safe(code: ErrorCode) -> SafeError {
    SafeError {
        code,
        retry_at: None,
    }
}

pub struct ProviderService {
    connection: Arc<dyn ApplicationConnection>,
    registered: Mutex<Option<(ActiveContext, Arc<ProviderRegistry>)>>,
    stopped: AtomicBool,
}

impl ProviderService {
    pub fn new(connection: Arc<dyn ApplicationConnection>) -> Arc<Self> {
        Arc::new(Self {
            connection,
            registered: Mutex::new(None),
            stopped: AtomicBool::new(false),
        })
    }
    pub fn setup() -> Arc<Self> {
        Self::new(Arc::new(SetupConnection(None)))
    }
    pub fn failed(diagnostic: SafeError) -> Arc<Self> {
        Self::new(Arc::new(SetupConnection(Some(diagnostic))))
    }
    pub fn context(&self) -> Option<ActiveContext> {
        if self.stopped.load(Ordering::Acquire) {
            None
        } else {
            self.connection.context()
        }
    }
    pub fn check(&self, expected: &ActiveContext) -> Result<(), SafeError> {
        let actual = self
            .context()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        if actual.scope != expected.scope {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        if actual != *expected {
            return Err(safe(ErrorCode::StaleContext));
        }
        expected
            .validate()
            .map_err(|_| safe(ErrorCode::ScopeMismatch))
    }
    pub fn check_optional(&self, expected: Option<&ActiveContext>) -> Result<(), SafeError> {
        match (expected, self.context()) {
            (Some(expected), _) => self.check(expected),
            (None, None) => Ok(()),
            (None, Some(_)) => Err(safe(ErrorCode::StaleContext)),
        }
    }
    async fn registration(
        &self,
        context: &ActiveContext,
    ) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        self.check(context)?;
        let mut registered = self.registered.lock().await;
        self.check(context)?;
        if registered.as_ref().is_none_or(|(old, _)| old != context) {
            if let Some((old, registry)) = registered.take()
                && let Ok(provider) = registry.get(&old.scope.provider)
                && let Ok(lifecycle) = provider.reviewed_lifecycle()
            {
                lifecycle.stop().await;
            }
            let provider = self.connection.registration().await?;
            self.check(context)?;
            let registry = Arc::new(ProviderRegistry::default());
            registry
                .register(provider)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
            *registered = Some((context.clone(), registry));
        }
        registered
            .as_ref()
            .unwrap()
            .1
            .get(&context.scope.provider)
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))
    }
    pub async fn bootstrap(&self, raw: Value) -> Result<Value, SafeError> {
        validate_version(&raw)?;
        let request: BootstrapRequest<Empty> = decode(raw)?;
        // Discovery has no pre-existing context on a newly opened window. A
        // supplied context is still binding, and the captured result is rechecked.
        if let Some(expected) = request.context.as_ref() {
            self.check(expected)?;
        }
        let context = self.context();
        let mut snapshot = self.connection.bootstrap()?;
        if context.is_none()
            && let Some(store) = self.connection.store()
        {
            // Absence of verified identity at cold start is not evidence that
            // persisted work belongs to a foreign or interrupted live session.
            // Keep its cursor/status/deadline untouched until exact-scope recovery.
            snapshot.recent_jobs = store
                .snapshot()
                .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?
                .jobs;
        }
        // No catalog lookup is performed by bootstrap or identity retry.
        if let Some(context) = &context {
            let provider = self.registration(context).await?;
            let lifecycle = provider
                .reviewed_lifecycle()
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
            lifecycle.recover(context).await?;
            self.check(context)?;
            snapshot.recent_jobs = lifecycle.jobs(context).await?;
            self.check(context)?;
            self.validate_jobs(context, &snapshot.recent_jobs)?;
        }
        if self.context() != context {
            return Err(safe(ErrorCode::StaleContext));
        }
        encode(BootstrapResponse {
            contract_version: 2,
            context,
            payload: snapshot,
        })
    }
    pub async fn auth(&self, raw: Value, retry: bool) -> Result<Value, SafeError> {
        validate_version(&raw)?;
        let request: BootstrapRequest<Value> = decode(raw)?;
        self.check_optional(request.context.as_ref())?;
        if retry {
            let _: Empty = decode(request.payload)?;
            self.connection.retry_identity().await?;
        } else {
            self.connection.auth(decode(request.payload)?).await?;
        }
        // Auth may legitimately establish a context. A captured old context may not be rebound.
        if let Some(expected) = request.context.as_ref() {
            self.check(expected)?;
        }
        encode(BootstrapResponse {
            contract_version: 2,
            context: self.context(),
            payload: self.connection.bootstrap()?,
        })
    }
    pub async fn active(&self, operation: &str, raw: Value) -> Result<Value, SafeError> {
        validate_version(&raw)?;
        if raw.get("context").is_none_or(Value::is_null) {
            return Err(safe(ErrorCode::IdentityUnavailable));
        }
        let request: CommandRequest<Value> = decode(raw)?;
        let context = request.context;
        self.check(&context)?;
        let provider = self.registration(&context).await?;
        let validator = provider
            .payload_validator()
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        self.check(&context)?;
        let payload = match operation {
            "snapshot" => {
                let _: Empty = decode(request.payload)?;
                let query = provider
                    .application_query()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                let chats = query.conversations(&context).await?;
                self.check(&context)?;
                for chat in &chats {
                    chat.validate(&context.scope)
                        .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                    validator
                        .validate_resource(&chat.resource)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                }
                let mut snapshot = self.connection.bootstrap()?;
                snapshot.chats = chats;
                snapshot.recent_jobs = provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .jobs(&context)
                    .await?;
                self.check(&context)?;
                self.validate_jobs(&context, &snapshot.recent_jobs)?;
                encode(snapshot)?
            }
            "search" => {
                let query: SearchRequest = decode(request.payload)?;
                validate_refs(
                    &context,
                    &query.conversations,
                    validator.as_ref(),
                    Some(ResourceKind::Conversation),
                )?;
                if query.limit == 0 || query.limit > 10_000 {
                    return Err(safe(ErrorCode::UnsupportedSchema));
                }
                let page = provider
                    .application_query()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .search_filtered(&context, query)
                    .await?;
                self.check(&context)?;
                for item in &page.items {
                    item.validate(&context.scope)
                        .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                    validator
                        .validate_resource(&item.resource)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                }
                encode(page)?
            }
            "refresh" => {
                let request: RefreshRequest = decode(request.payload)?;
                validate_refs(
                    &context,
                    &request.conversations,
                    validator.as_ref(),
                    Some(ResourceKind::Conversation),
                )?;
                if request.conversations.len() > 1000 {
                    return Err(safe(ErrorCode::UnsupportedSchema));
                }
                let unique: BTreeMap<_, _> = request
                    .conversations
                    .into_iter()
                    .map(|r| (r.id, r))
                    .collect();
                let chats = provider
                    .application_query()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .refresh(&context, unique.values().cloned().collect())
                    .await?;
                self.check(&context)?;
                let mut seen = std::collections::HashSet::new();
                for chat in &chats {
                    chat.validate(&context.scope)
                        .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                    if !seen.insert(chat.id)
                        || unique
                            .get(chat.id.as_uuid())
                            .is_none_or(|r| r.resource != chat.resource)
                    {
                        return Err(safe(ErrorCode::ScopeMismatch));
                    }
                    validator
                        .validate_resource(&chat.resource)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                }
                encode(chats)?
            }
            "prepare_selection" | "prepare_intent" | "intents" => {
                let intent: PrepareIntent = if operation == "prepare_selection" {
                    let selection: SelectionRequest = decode(request.payload)?;
                    PrepareIntent {
                        action_id: "selected_messages".into(),
                        targets: selection.message_refs,
                        actor: None,
                    }
                } else {
                    decode(request.payload)?
                };
                validate_refs(&context, &intent.targets, validator.as_ref(), None)?;
                if let Some(actor) = &intent.actor {
                    validate_refs(
                        &context,
                        std::slice::from_ref(actor),
                        validator.as_ref(),
                        Some(ResourceKind::Actor),
                    )?;
                }
                let lifecycle = provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                if operation == "intents" {
                    let intents = lifecycle.intents(&context, intent.targets).await?;
                    self.check(&context)?;
                    validate_intents(&intents)?;
                    encode(intents)?
                } else {
                    let plan = lifecycle.prepare(&context, intent).await?;
                    self.check(&context)?;
                    plan.validate()
                        .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                    if plan.scope != context.scope {
                        return Err(safe(ErrorCode::ScopeMismatch));
                    }
                    validator
                        .validate_recipe(&plan)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                    let stored = self.store_plan(&context, plan.id)?;
                    if stored != plan {
                        return Err(safe(ErrorCode::StatePersistenceFailed));
                    }
                    encode(plan)?
                }
            }
            "authorize" => {
                let plan: ReviewedPlanRef = decode(request.payload)?;
                let stored = self.store_plan(&context, plan.plan_id)?;
                if stored.fingerprint != plan.fingerprint {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .authorize(&context, plan)
                    .await?;
                encode(Empty {})?
            }
            "execute" => {
                let start: StartReviewed = decode(request.payload)?;
                let stored = self.store_plan(&context, start.plan_id)?;
                if stored.fingerprint != start.fingerprint {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let job = provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .start(&context, start)
                    .await?;
                job.validate(&stored)
                    .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                encode(job)?
            }
            "jobs" => {
                let _: Empty = decode(request.payload)?;
                let jobs = provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .jobs(&context)
                    .await?;
                self.check(&context)?;
                self.validate_jobs(&context, &jobs)?;
                encode(jobs)?
            }
            "cancel" => {
                let cancel: CancelRequest = decode(request.payload)?;
                if cancel.job_id.is_nil() {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let store = self
                    .connection
                    .store()
                    .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
                let state = store
                    .snapshot()
                    .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?;
                let job = state
                    .jobs
                    .iter()
                    .find(|j| j.id == cancel.job_id)
                    .ok_or_else(|| safe(ErrorCode::NotFound))?;
                if job.scope != context.scope {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let result = provider
                    .reviewed_lifecycle()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
                    .cancel(&context, cancel.job_id)
                    .await?;
                result
                    .validate(&self.store_plan(&context, result.plan_id)?)
                    .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
                encode(result)?
            }
            _ => return Err(safe(ErrorCode::UnsupportedContractVersion)),
        };
        self.check(&context)?;
        encode(CommandResponse {
            contract_version: 2,
            context,
            payload,
        })
    }
    fn validate_jobs(
        &self,
        context: &ActiveContext,
        jobs: &[retract_domain::ScopedJobRecord],
    ) -> Result<(), SafeError> {
        for job in jobs {
            job.validate(&self.store_plan(context, job.plan_id)?)
                .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        }
        Ok(())
    }
    fn store_plan(
        &self,
        context: &ActiveContext,
        id: uuid::Uuid,
    ) -> Result<retract_domain::RemediationPlan, SafeError> {
        let state = self
            .connection
            .store()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?
            .snapshot()
            .map_err(|_| safe(ErrorCode::StatePersistenceFailed))?;
        let plan = state
            .plans
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| safe(ErrorCode::NotFound))?;
        if plan.scope != context.scope {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        plan.validate()
            .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        Ok(plan)
    }
    pub async fn has_workers(&self) -> bool {
        let registered = self.registered.lock().await;
        if let Some((context, registry)) = &*registered
            && let Ok(provider) = registry.get(&context.scope.provider)
            && let Ok(lifecycle) = provider.reviewed_lifecycle()
        {
            return lifecycle.has_workers().await;
        }
        false
    }
    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut registered = self.registered.lock().await;
        if let Some((context, registry)) = registered.take()
            && let Ok(provider) = registry.get(&context.scope.provider)
            && let Ok(lifecycle) = provider.reviewed_lifecycle()
        {
            lifecycle.stop().await;
        }
        self.connection.shutdown().await;
    }
}

pub fn validate_version(raw: &Value) -> Result<(), SafeError> {
    if raw.get("contractVersion").and_then(Value::as_u64) != Some(2) {
        return Err(safe(ErrorCode::UnsupportedContractVersion));
    }
    Ok(())
}

fn validate_intents(intents: &[IntentDescriptor]) -> Result<(), SafeError> {
    let bounded = |text: &str, max: usize| {
        !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
    };
    let mut ids = std::collections::HashSet::new();
    if intents.len() > 1000 {
        return Err(safe(ErrorCode::UnsupportedSchema));
    }
    for intent in intents {
        if !bounded(&intent.action_id, 128)
            || !bounded(&intent.label, 256)
            || !ids.insert(&intent.action_id)
            || intent.descriptors.is_empty()
            || intent.descriptors.len() > 100_000
        {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        for descriptor in &intent.descriptors {
            descriptor
                .validate()
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        }
    }
    Ok(())
}
pub fn decode<T: DeserializeOwned>(raw: Value) -> Result<T, SafeError> {
    serde_json::from_value(raw).map_err(|_| safe(ErrorCode::ScopeMismatch))
}
pub fn encode<T: Serialize>(value: T) -> Result<Value, SafeError> {
    serde_json::to_value(value).map_err(|_| safe(ErrorCode::StatePersistenceFailed))
}
pub fn validate_refs(
    context: &ActiveContext,
    refs: &[ScopedResourceRef],
    validator: &dyn crate::persistence::ProviderPayloadValidator,
    kind: Option<ResourceKind>,
) -> Result<(), SafeError> {
    if refs.len() > 100_000 {
        return Err(safe(ErrorCode::UnsupportedSchema));
    }
    let mut seen = BTreeMap::new();
    for reference in refs {
        reference
            .validate(&context.scope)
            .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        if kind.is_some_and(|kind| reference.resource.resource_kind != kind) {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        if seen
            .insert(reference.id, reference)
            .is_some_and(|old| old != reference)
        {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        validator
            .validate_resource(&reference.resource)
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
    }
    Ok(())
}

struct SetupConnection(Option<SafeError>);
#[async_trait::async_trait]
impl ApplicationConnection for SetupConnection {
    fn context(&self) -> Option<ActiveContext> {
        None
    }
    fn store(&self) -> Option<Arc<crate::persistence::FoundationStore>> {
        None
    }
    fn bootstrap(&self) -> Result<BootstrapSnapshot, SafeError> {
        Ok(BootstrapSnapshot {
            identity: self
                .0
                .clone()
                .map_or(IdentityStatus::Unavailable, |diagnostic| {
                    IdentityStatus::Failed { diagnostic }
                }),
            auth: None,
            catalog: CatalogProgress::default(),
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: vec![],
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        Err(safe(ErrorCode::IdentityUnavailable))
    }
    async fn auth(&self, _: AuthRequest) -> Result<(), SafeError> {
        Err(safe(ErrorCode::AuthenticationRequired))
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        Err(safe(ErrorCode::AuthenticationRequired))
    }
    async fn shutdown(&self) {}
}
