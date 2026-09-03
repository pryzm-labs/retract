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
