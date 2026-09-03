use crate::{
    ActionDescriptor, ActionKind, Availability, ConfirmationTier, DomainError, ErrorCode,
    SafeError, Scope, ScopedResourceRef, VersionedPayload,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionStep {
    pub descriptor: ActionDescriptor,
    pub targets: Vec<ScopedResourceRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmationRequirements {
    pub tier: ConfirmationTier,
    pub acknowledgement_required: bool,
    pub owner_auth_required: bool,
    pub exact_text: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    RequiresNewReview,
    ResumeFrozenTargets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemediationPlan {
    pub id: Uuid,
    pub scope: Scope,
    pub steps: Vec<ActionStep>,
    pub targets: Vec<ScopedResourceRef>,
    pub confirmation: ConfirmationRequirements,
    pub recipe: VersionedPayload,
    pub restart_policy: RestartPolicy,
    pub created_at: DateTime<Utc>,
    pub fingerprint: String,
}
impl RemediationPlan {
    /// Seals only the neutral envelope. An adapter must validate its typed,
    /// content-free recipe, schema support and exact recipe/target agreement
    /// before publishing or executing this plan. A digest is not authorization.
    pub fn seal(&mut self) -> Result<(), DomainError> {
        let mut canonical = self.clone();
        canonical.normalize()?;
        canonical.fingerprint = canonical.compute_fingerprint()?;
        *self = canonical;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), DomainError> {
        let mut canonical = self.clone();
        canonical.normalize()?;
        if canonical.compute_fingerprint()? != self.fingerprint {
            return Err(DomainError::FingerprintMismatch);
        }
        Ok(())
    }
    fn normalize(&mut self) -> Result<(), DomainError> {
        if self.id.is_nil()
            || self.steps.is_empty()
            || self.steps.len() > 1000
            || self.targets.is_empty()
            || self.targets.len() > 100_000
        {
            return Err(DomainError::InvalidPlan);
        }
        self.recipe.validate()?;
        // A recipe cannot smuggle a second fingerprint into the binding.
        if contains_fingerprint(&self.recipe.payload) {
            return Err(DomainError::InvalidPlan);
        }
        self.targets = canonical_targets(&self.targets, &self.scope)?;
        let mut step_targets = Vec::new();
        let mut tier = ConfirmationTier::Low;
        for step in &mut self.steps {
            step.descriptor.validate()?;
            if !matches!(
                step.descriptor.availability,
                Availability::Executable | Availability::LivePreflightRequired
            ) || step.targets.is_empty()
                || step.targets.len() > 100_000
            {
                return Err(DomainError::InvalidPlan);
            }
            tier = tier.max(step.descriptor.confirmation_tier);
            if self.restart_policy == RestartPolicy::ResumeFrozenTargets
                && (!matches!(
                    step.descriptor.kind,
                    ActionKind::DeleteRemoteItem | ActionKind::RemoveForCurrentAccount
                ) || step
                    .descriptor
                    .advisory
                    .as_ref()
                    .is_some_and(|a| a.cost_bearing))
            {
                return Err(DomainError::InvalidPlan);
            }
            step.targets = canonical_targets(&step.targets, &self.scope)?;
            step_targets.extend(step.targets.iter().cloned());
        }
        // Check conflicts across both lists before comparing membership.
        let mut all = self.targets.clone();
        all.extend(step_targets.iter().cloned());
        canonical_targets(&all, &self.scope)?;
        if canonical_targets(&step_targets, &self.scope)? != self.targets {
            return Err(DomainError::InvalidPlan);
        }
        if self.confirmation.tier < tier
            || !self.confirmation.acknowledgement_required
            || !self.confirmation.owner_auth_required
            || (self.confirmation.tier >= ConfirmationTier::High
                && self
                    .confirmation
                    .exact_text
                    .as_ref()
                    .is_none_or(|s| !crate::identity::bounded_text(s, 4096)))
            || self
                .confirmation
                .exact_text
                .as_ref()
                .is_some_and(|s| !crate::identity::bounded_text(s, 4096))
        {
            return Err(DomainError::InvalidConfirmation);
        }
        Ok(())
    }
    fn compute_fingerprint(&self) -> Result<String, DomainError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Binding<'a> {
            fingerprint_schema: &'static str,
            id: Uuid,
            scope: &'a Scope,
            steps: &'a [ActionStep],
            targets: &'a [ScopedResourceRef],
            confirmation: &'a ConfirmationRequirements,
            recipe: &'a VersionedPayload,
            restart_policy: RestartPolicy,
            created_at: DateTime<Utc>,
        }
        let value = serde_json::to_value(Binding {
            fingerprint_schema: "retract-plan-v1",
            id: self.id,
            scope: &self.scope,
            steps: &self.steps,
            targets: &self.targets,
            confirmation: &self.confirmation,
            recipe: &self.recipe,
            restart_policy: self.restart_policy,
            created_at: self.created_at,
        })
        .map_err(|_| DomainError::InvalidPlan)?;
        let bytes = canonical_json(&value)?;
        let digest = Sha256::digest(bytes);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(format!("sha256-v1:{hex}"))
    }
}

fn contains_fingerprint(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == "fingerprint" || contains_fingerprint(value)),
        serde_json::Value::Array(items) => items.iter().any(contains_fingerprint),
        _ => false,
    }
}

fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, DomainError> {
    fn ordered(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), ordered(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(ordered).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_vec(&ordered(value)).map_err(|_| DomainError::InvalidPlan)
}

fn canonical_targets(
    targets: &[ScopedResourceRef],
    scope: &Scope,
) -> Result<Vec<ScopedResourceRef>, DomainError> {
    let mut by_id = BTreeMap::new();
    for target in targets {
        target.validate(scope)?;
        if let Some(previous) = by_id.insert(target.id, target.clone())
            && previous != *target
        {
            return Err(DomainError::ConflictingReference);
        }
    }
    Ok(by_id.into_values().collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Blocked,
    Completed,
    Partial,
    Failed,
    Cancelled,
}
impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Failed | Self::Cancelled
        )
    }
}
/// `selected` includes reviewed unavailable items. `eligible` counts executable
/// targets; skipped may include selections outside eligible. Never add skipped
/// to eligible-result counters to infer an invariant for legacy history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobCounters {
    pub selected: u64,
    pub eligible: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub uncertain: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedJobRecord {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub scope: Scope,
    pub dirty_refs: Vec<ScopedResourceRef>,
    pub status: JobStatus,
    pub counters: JobCounters,
    pub next_batch: u64,
    pub retry_at: Option<DateTime<Utc>>,
    pub diagnostics: Vec<SafeError>,
    pub started_authorized: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl ScopedJobRecord {
    /// The adapter additionally validates its bounded batch cursor and restart
    /// eligibility. Neutral records never infer a native batch count from IDs.
    pub fn validate(&self, plan: &RemediationPlan) -> Result<(), DomainError> {
        plan.validate()?;
        if self.id.is_nil()
            || self.plan_id != plan.id
            || self.scope != plan.scope
            || self.updated_at < self.created_at
            || (matches!(
                self.status,
                JobStatus::Running | JobStatus::Completed | JobStatus::Partial
            ) && !self.started_authorized)
        {
            return Err(DomainError::InvalidJob);
        }
        canonical_targets(&self.dirty_refs, &self.scope)?;
        if (self.status.is_terminal() || self.status == JobStatus::Blocked)
            && self.retry_at.is_some()
        {
            return Err(DomainError::InvalidJob);
        }
        if self.status == JobStatus::Blocked
            && !self.diagnostics.iter().any(|d| {
                matches!(
                    d.code,
                    ErrorCode::IdentityUnavailable
                        | ErrorCode::ScopeMismatch
                        | ErrorCode::StaleContext
                )
            })
        {
            return Err(DomainError::InvalidJob);
        }
        let accounted_eligible = self
            .counters
            .deleted
            .checked_add(self.counters.failed)
            .and_then(|count| count.checked_add(self.counters.uncertain))
            .ok_or(DomainError::InvalidJob)?;
        if self.counters.eligible > self.counters.selected
            || self.counters.skipped > self.counters.selected
            || accounted_eligible > self.counters.eligible
            || (self.status == JobStatus::Completed
                && (self.counters.failed > 0 || self.counters.uncertain > 0))
        {
            return Err(DomainError::InvalidJob);
        }
        if self.counters.uncertain > 0 && !self.status.is_terminal() {
            return Err(DomainError::InvalidJob);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyTerminalStatus {
    Completed,
    Partial,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyOperation {
    SelectedMessages,
    DeleteMyMessages,
    ClearHistory,
    ClearHistoryAndLeave,
    DeleteAllMessagesAndLeave,
    RemoveChatForSelf,
    DeleteBySender,
    DeleteGroup,
    LeaveChat,
}
/// Historical, deliberately unbound and non-executable. No account/source,
/// plan recipe, retry schedule or provider-native message data is retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyHistoryRecord {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub operation: LegacyOperation,
    pub status: LegacyTerminalStatus,
    pub total: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub next_batch: u64,
    pub diagnostics: Vec<SafeError>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
