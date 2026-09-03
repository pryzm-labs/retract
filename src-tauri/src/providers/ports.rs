use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use retract_domain::{
    ActionDescriptor, ActionKind, ActiveContext, ContentRecord, ConversationRecord, ProviderError,
    ProviderKey, RemediationPlan, Scope, ScopedResourceRef, VersionedPayload,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapability {
    LiveConnection,
    ArchiveImport,
    ImportInspection,
    ConversationListing,
    ContentSearch,
    MediaMetadata,
    ExternalLocation,
    AutomaticRemediation,
    BulkRemediation,
    Verification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDescriptor {
    pub key: ProviderKey,
    pub display_name: String,
    pub capabilities: BTreeSet<ProviderCapability>,
}

impl ProviderDescriptor {
    pub fn validate(&self) -> Result<(), super::registry::ProviderRegistryError> {
        if self.display_name.trim().is_empty()
            || self.display_name.len() > 128
            || self.display_name.chars().any(char::is_control)
            || self.capabilities.is_empty()
        {
            return Err(super::registry::ProviderRegistryError::InvalidDescriptor(
                self.key.clone(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuery {
    pub scope: Scope,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentQuery {
    pub scope: Scope,
    pub query: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveRequest {
    pub scope: Scope,
    pub refs: Vec<ScopedResourceRef>,
}

#[async_trait]
pub trait QuerySource: Send + Sync {
    async fn list_conversations(
        &self,
        request: ConversationQuery,
    ) -> Result<Page<ConversationRecord>, ProviderError>;

    async fn search(&self, request: ContentQuery) -> Result<Page<ContentRecord>, ProviderError>;

    async fn resolve(&self, request: ResolveRequest) -> Result<Vec<ContentRecord>, ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionRequest {
    pub context: ActiveContext,
    pub targets: Vec<ScopedResourceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionResult {
    pub target: ScopedResourceRef,
    pub descriptors: Vec<ActionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightRequest {
    pub context: ActiveContext,
    pub plan: RemediationPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightResult {
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub descriptors: Vec<ActionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBatch {
    pub context: ActiveContext,
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub step_index: u32,
    pub action: ActionKind,
    pub targets: Vec<ScopedResourceRef>,
    pub recipe: VersionedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchOutcome {
    Confirmed,
    Skipped,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchItemResult {
    pub target: ScopedResourceRef,
    pub outcome: BatchOutcome,
    pub diagnostic: Option<retract_domain::SafeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchResult {
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub items: Vec<BatchItemResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationRequest {
    pub context: ActiveContext,
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub action: ActionKind,
    pub targets: Vec<ScopedResourceRef>,
    pub recipe: VersionedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Confirmed,
    NotApplied,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationItem {
    pub target: ScopedResourceRef,
    pub state: VerificationState,
    pub diagnostic: Option<retract_domain::SafeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationResult {
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub items: Vec<VerificationItem>,
}

#[async_trait]
pub trait RemediationProvider: Send + Sync {
    async fn actions_for(&self, request: ActionRequest)
    -> Result<Vec<ActionResult>, ProviderError>;
    async fn preflight(&self, request: PreflightRequest) -> Result<PreflightResult, ProviderError>;
    async fn execute_batch(&self, batch: ExecutionBatch) -> Result<BatchResult, ProviderError>;
    async fn verify(
        &self,
        request: VerificationRequest,
    ) -> Result<VerificationResult, ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionSnapshot {
    pub context: Option<ActiveContext>,
    pub state: retract_domain::ConnectionState,
}

#[async_trait]
pub trait LiveConnection: Send + Sync {
    fn snapshot(&self) -> ConnectionSnapshot;
    async fn reconnect(&self) -> Result<ConnectionSnapshot, ProviderError>;
    async fn disconnect(&self) -> Result<ConnectionSnapshot, ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInspectionRequest {
    pub provider: ProviderKey,
    pub source: VersionedPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInspection {
    pub provider: ProviderKey,
    pub source_schema: String,
    pub source_version: u16,
    pub conversations: u64,
    pub items: u64,
    pub warnings: Vec<retract_domain::SafeError>,
}

#[async_trait]
pub trait ImportInspector: Send + Sync {
    async fn inspect(
        &self,
        request: ImportInspectionRequest,
    ) -> Result<ImportInspection, ProviderError>;
}

pub trait ProviderRegistration: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    fn reviewed_lifecycle(
        &self,
    ) -> Result<Arc<dyn ReviewedLifecycle>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::AutomaticRemediation,
        ))
    }

    fn application_query(
        &self,
    ) -> Result<Arc<dyn ApplicationQuery>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::ContentSearch,
        ))
    }

    fn payload_validator(
        &self,
    ) -> Result<
        Arc<dyn crate::persistence::ProviderPayloadValidator>,
        super::registry::ProviderRegistryError,
    > {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::ContentSearch,
        ))
    }

    fn query_source(&self) -> Result<Arc<dyn QuerySource>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::ContentSearch,
        ))
    }

    fn remediation(
        &self,
    ) -> Result<Arc<dyn RemediationProvider>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::AutomaticRemediation,
        ))
    }

    fn live_connection(
        &self,
    ) -> Result<Arc<dyn LiveConnection>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::LiveConnection,
        ))
    }

    fn import_inspector(
        &self,
    ) -> Result<Arc<dyn ImportInspector>, super::registry::ProviderRegistryError> {
        Err(super::registry::ProviderRegistryError::unsupported(
            ProviderCapability::ImportInspection,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareIntent {
    pub action_id: String,
    pub targets: Vec<ScopedResourceRef>,
    pub actor: Option<ScopedResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedPlanRef {
    pub plan_id: Uuid,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartReviewed {
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub irreversible_acknowledged: bool,
    pub typed_chat_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentDescriptor {
    pub action_id: String,
    pub label: String,
    pub requires_actor: bool,
    pub descriptors: Vec<ActionDescriptor>,
}

/// Whole reviewed lifecycle; the raw batch port cannot authorize a native call.
#[async_trait]
pub trait ReviewedLifecycle: Send + Sync {
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, retract_domain::SafeError>;
    async fn prepare(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
    ) -> Result<RemediationPlan, retract_domain::SafeError>;
    async fn authorize(
        &self,
        context: &ActiveContext,
        plan: ReviewedPlanRef,
    ) -> Result<(), retract_domain::SafeError>;
    async fn start(
        &self,
        context: &ActiveContext,
        request: StartReviewed,
    ) -> Result<retract_domain::ScopedJobRecord, retract_domain::SafeError>;
    async fn jobs(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<retract_domain::ScopedJobRecord>, retract_domain::SafeError>;
    async fn cancel(
        &self,
        context: &ActiveContext,
        job_id: Uuid,
    ) -> Result<retract_domain::ScopedJobRecord, retract_domain::SafeError>;
    async fn recover(&self, context: &ActiveContext) -> Result<(), retract_domain::SafeError>;
    async fn has_workers(&self) -> bool;
    async fn stop(&self);
}

/// Extended scoped query boundary preserves provider-owned filters and targeted
/// conversation resolution without widening an isolated refresh into a catalog read.
#[async_trait]
pub trait ApplicationQuery: Send + Sync {
    async fn conversations(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<ConversationRecord>, retract_domain::SafeError>;
    async fn search_filtered(
        &self,
        context: &ActiveContext,
        request: crate::compatibility::model_v2::SearchRequest,
    ) -> Result<Page<ContentRecord>, retract_domain::SafeError>;
    async fn refresh(
        &self,
        context: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<ConversationRecord>, retract_domain::SafeError>;
}

/// Provider-owned connection/bootstrap. Only authenticated backend state may
/// supply context or a registration; no command supplies an account binding.
#[async_trait]
pub trait ApplicationConnection: Send + Sync {
    fn context(&self) -> Option<ActiveContext>;
    fn store(&self) -> Option<Arc<crate::persistence::FoundationStore>>;
    fn bootstrap(
        &self,
    ) -> Result<crate::compatibility::model_v2::BootstrapSnapshot, retract_domain::SafeError>;
    async fn registration(
        &self,
    ) -> Result<Arc<dyn ProviderRegistration>, retract_domain::SafeError>;
    async fn auth(
        &self,
        request: crate::compatibility::model_v2::AuthRequest,
    ) -> Result<(), retract_domain::SafeError>;
    async fn retry_identity(&self) -> Result<(), retract_domain::SafeError>;
    async fn shutdown(&self);
}
