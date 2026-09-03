use std::collections::{HashMap, HashSet};

use retract_domain::{
    AccountRecord, ProviderResourceRef, RemediationPlan, ScopedJobRecord, SourceRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppError;

pub const FOUNDATION_SCHEMA_VERSION: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreBinding {
    pub provider: retract_domain::ProviderKey,
    pub profile: String,
}

impl StoreBinding {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if self.profile.trim().is_empty()
            || self.profile.len() > 256
            || self.profile.chars().any(char::is_control)
        {
            return Err(invalid_state("invalid store binding"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyStoreFormat {
    Rtrct01,
    Rtrct02,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationProvenance {
    pub source_format: LegacyStoreFormat,
    pub source_sha256: String,
    pub source_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyHistoryEntry {
    pub record: retract_domain::LegacyHistoryRecord,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FoundationState {
    pub schema_version: u16,
    pub binding: StoreBinding,
    pub identities: Vec<AccountRecord>,
    pub sources: Vec<SourceRecord>,
    pub plans: Vec<RemediationPlan>,
    pub jobs: Vec<ScopedJobRecord>,
    pub legacy_history: Vec<LegacyHistoryEntry>,
    pub migration: Option<MigrationProvenance>,
}

impl FoundationState {
    pub fn empty(binding: StoreBinding) -> Self {
        Self {
            schema_version: FOUNDATION_SCHEMA_VERSION,
            binding,
            identities: Vec::new(),
            sources: Vec::new(),
            plans: Vec::new(),
            jobs: Vec::new(),
            legacy_history: Vec::new(),
            migration: None,
        }
    }

    pub fn validate(&self, expected_binding: &StoreBinding) -> Result<(), AppError> {
        expected_binding.validate()?;
        if self.schema_version != FOUNDATION_SCHEMA_VERSION || &self.binding != expected_binding {
            return Err(invalid_state("foundation state binding does not match"));
        }

        let mut identities = HashMap::new();
        for identity in &self.identities {
            identity
                .validate()
                .map_err(|_| invalid_state("invalid account identity"))?;
            if identity.provider != expected_binding.provider
                || identities.insert(identity.id, identity).is_some()
                || !content_free_payload(&identity.native_identity.payload)
                || identity
                    .avatar
                    .as_ref()
                    .is_some_and(|avatar| !content_free_payload(&avatar.payload))
            {
                return Err(invalid_state("invalid or duplicate account identity"));
            }
        }

        let mut sources = HashMap::new();
        for source in &self.sources {
            let account = identities
                .get(&source.account_id)
                .ok_or_else(|| invalid_state("source account is missing"))?;
            source
                .validate(account)
                .map_err(|_| invalid_state("invalid source relationship"))?;
            if source.provider != expected_binding.provider
                || sources.insert(source.id, source).is_some()
                || !content_free_payload(&source.schema_profile.payload)
            {
                return Err(invalid_state("invalid or duplicate source"));
            }
        }

        let mut plans = HashMap::new();
        let mut resources: HashMap<Uuid, &ProviderResourceRef> = HashMap::new();
        for plan in &self.plans {
            plan.validate()
                .map_err(|_| invalid_state("invalid remediation plan"))?;
            validate_scope_owner(&plan.scope, expected_binding, &identities, &sources)?;
            if plans.insert(plan.id, plan).is_some() || !content_free_payload(&plan.recipe.payload)
            {
                return Err(invalid_state("invalid or duplicate remediation plan"));
            }
            for target in &plan.targets {
                match resources.insert(target.id, &target.resource) {
                    Some(previous) if previous != &target.resource => {
                        return Err(invalid_state("conflicting resource identity"));
                    }
                    _ => {}
                }
            }
        }

        let mut job_ids = HashSet::new();
        for job in &self.jobs {
            let plan = plans
                .get(&job.plan_id)
                .ok_or_else(|| invalid_state("job plan is missing"))?;
            job.validate(plan)
                .map_err(|_| invalid_state("invalid scoped job"))?;
            validate_scope_owner(&job.scope, expected_binding, &identities, &sources)?;
            if !job_ids.insert(job.id) || job.next_batch > plan_batch_count(plan)? {
                return Err(invalid_state("invalid or duplicate job"));
            }
            for target in &job.dirty_refs {
                match resources.insert(target.id, &target.resource) {
                    Some(previous) if previous != &target.resource => {
                        return Err(invalid_state("conflicting resource identity"));
                    }
                    _ => {}
                }
            }
        }

        let mut legacy_ids = HashSet::new();
        for entry in &self.legacy_history {
            let record = &entry.record;
            if entry.executable
                || record.id.is_nil()
                || record.plan_id.is_nil()
                || record.updated_at < record.created_at
                || !legacy_ids.insert(record.id)
                || job_ids.contains(&record.id)
            {
                return Err(invalid_state("invalid legacy history"));
            }
            let requires_review = record
                .diagnostics
                .iter()
                .any(|error| error.code == retract_domain::ErrorCode::MigrationRequiresNewReview);
            if requires_review
                && !matches!(
                    record.status,
                    retract_domain::LegacyTerminalStatus::Failed
                        | retract_domain::LegacyTerminalStatus::Partial
                )
            {
                return Err(invalid_state("invalid stopped legacy history"));
            }
        }
        if !self.legacy_history.is_empty() && self.migration.is_none() {
            return Err(invalid_state("legacy history lacks migration provenance"));
        }
        if let Some(migration) = &self.migration
            && (migration.source_bytes == 0 || !valid_sha256(&migration.source_sha256))
        {
            return Err(invalid_state("invalid migration provenance"));
        }
        Ok(())
    }
}

fn validate_scope_owner<'a>(
    scope: &retract_domain::Scope,
    binding: &StoreBinding,
    identities: &HashMap<retract_domain::AccountId, &'a AccountRecord>,
    sources: &HashMap<retract_domain::SourceId, &'a SourceRecord>,
) -> Result<(), AppError> {
    if scope.provider != binding.provider {
        return Err(invalid_state("row provider does not match store binding"));
    }
    let account = identities
        .get(&scope.account_id)
        .ok_or_else(|| invalid_state("row account is missing"))?;
    let source = sources
        .get(&scope.source_id)
        .ok_or_else(|| invalid_state("row source is missing"))?;
    if account.provider != scope.provider
        || source.account_id != scope.account_id
        || source.provider != scope.provider
    {
        return Err(invalid_state("row source ownership does not match"));
    }
    Ok(())
}

fn plan_batch_count(plan: &RemediationPlan) -> Result<u64, AppError> {
    plan.steps.iter().try_fold(0_u64, |total, step| {
        let targets = u64::try_from(step.targets.len())
            .map_err(|_| invalid_state("plan batch count overflow"))?;
        let size = u64::from(step.descriptor.batch.max_targets);
        let batches = targets
            .checked_add(size - 1)
            .and_then(|value| value.checked_div(size))
            .ok_or_else(|| invalid_state("plan batch count overflow"))?;
        total
            .checked_add(batches)
            .ok_or_else(|| invalid_state("plan batch count overflow"))
    })
}

fn content_free_payload(payload: &Value) -> bool {
    fn visit(value: &Value, depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => true,
            Value::String(value) => value.len() <= 4096 && !value.chars().any(char::is_control),
            Value::Array(values) => {
                values.len() <= 100_000 && values.iter().all(|value| visit(value, depth + 1))
            }
            Value::Object(values) => {
                values.len() <= 10_000
                    && values.iter().all(|(key, value)| {
                        let normalized = key
                            .chars()
                            .filter(|character| character.is_ascii_alphanumeric())
                            .flat_map(char::to_lowercase)
                            .collect::<String>();
                        !matches!(
                            normalized.as_str(),
                            "text"
                                | "body"
                                | "messagebody"
                                | "caption"
                                | "preview"
                                | "filename"
                                | "attachmentname"
                                | "privateattachmentname"
                                | "filepath"
                                | "archivepath"
                                | "password"
                                | "token"
                                | "secret"
                                | "credential"
                                | "credentials"
                                | "apihash"
                                | "apikey"
                                | "accesskey"
                                | "authcode"
                                | "authvalue"
                                | "authorization"
                                | "session"
                                | "nativeerror"
                                | "rawerror"
                                | "providererror"
                                | "errormessage"
                                | "error"
                        ) && visit(value, depth + 1)
                    })
            }
        }
    }

    serde_json::to_vec(payload).is_ok_and(|bytes| bytes.len() <= 1_048_576) && visit(payload, 0)
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn invalid_state(message: &'static str) -> AppError {
    AppError::SecureStore(message.into())
}
