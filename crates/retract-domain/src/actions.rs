use crate::{DomainError, SafeError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    DeleteRemoteItem,
    RemoveForCurrentAccount,
    ClearConversation,
    LeaveConversation,
    DeleteConversation,
    DeleteByActor,
    OpenExternally,
    ManualRemediation,
    RemoveLocalImport,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedEffect {
    RemovedForAllParticipants,
    RemovedForCurrentAccountOnly,
    PublicContentRemoved,
    MembershipRemoved,
    ContainerDestroyed,
    LocalImportRemoved,
    ManualOrUnknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Executable,
    LivePreflightRequired,
    ManualOnly,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationTier {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchConstraints {
    pub max_targets: u32,
    pub max_parallel: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionAdvisory {
    pub cost_bearing: bool,
    pub rate_limited: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionDescriptor {
    pub id: String,
    pub kind: ActionKind,
    pub effect: ExpectedEffect,
    pub availability: Availability,
    pub unavailable_reason: Option<SafeError>,
    pub requires_live_preflight: bool,
    pub batch: BatchConstraints,
    pub confirmation_tier: ConfirmationTier,
    pub destructive: bool,
    pub irreversible: bool,
    pub advisory: Option<ActionAdvisory>,
}
impl ActionDescriptor {
    pub fn validate(&self) -> Result<(), DomainError> {
        if !crate::identity::bounded_text(&self.id, 128)
            || self.batch.max_targets == 0
            || self.batch.max_parallel == 0
            || (self.availability == Availability::Unavailable) != self.unavailable_reason.is_some()
            || (self.availability == Availability::LivePreflightRequired
                && !self.requires_live_preflight)
            || (self.effect == ExpectedEffect::ContainerDestroyed
                && (self.confirmation_tier != ConfirmationTier::Critical
                    || !self.destructive
                    || !self.irreversible))
        {
            return Err(DomainError::InvalidPlan);
        }
        Ok(())
    }
}
