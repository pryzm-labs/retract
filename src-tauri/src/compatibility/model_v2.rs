//! The sole identity-bearing application wire contract. Native locators remain opaque.
use retract_domain::{
    ActiveContext, ConversationRecord, LegacyHistoryRecord, SafeError, ScopedJobRecord,
    ScopedResourceRef, VersionedPayload,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRequest<T> {
    pub contract_version: u16,
    pub context: ActiveContext,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapRequest<T> {
    pub contract_version: u16,
    pub context: Option<ActiveContext>,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandResponse<T> {
    pub contract_version: u16,
    pub context: ActiveContext,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapResponse<T> {
    pub contract_version: u16,
    pub context: Option<ActiveContext>,
    pub payload: T,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum IdentityStatus {
    #[default]
    Unavailable,
    Pending,
    Ready,
    Failed {
        diagnostic: SafeError,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogProgress {
    pub phase: String,
    pub total: usize,
    pub processed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapSnapshot {
    pub identity: IdentityStatus,
    pub auth: Option<VersionedPayload>,
    pub catalog: CatalogProgress,
    pub chats: Vec<ConversationRecord>,
    pub recent_jobs: Vec<ScopedJobRecord>,
    pub legacy_history: Vec<LegacyHistoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionRequest {
    pub message_refs: Vec<ScopedResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshRequest {
    pub conversations: Vec<ScopedResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelRequest {
    pub job_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    pub conversations: Vec<ScopedResourceRef>,
    pub filters: Option<VersionedPayload>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthRequest {
    pub operation: String,
    pub value: Option<String>,
}
