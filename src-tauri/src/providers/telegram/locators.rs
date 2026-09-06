use retract_domain::{
    AccountId, AccountRecord, ActionKind, ActionStep, ProviderKey, ProviderResourceRef,
    RemediationPlan, ResourceKind, ScopedResourceRef, SourceKind, SourceRecord, VersionedPayload,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
};

pub const TELEGRAM_ACCOUNT_SCHEMA: &str = "telegram.account";
pub const TELEGRAM_CONVERSATION_SCHEMA: &str = "telegram.conversation";
pub const TELEGRAM_MESSAGE_SCHEMA: &str = "telegram.message";
pub const TELEGRAM_ACTOR_SCHEMA: &str = "telegram.actor";
pub const TELEGRAM_GROUPING_SCHEMA: &str = "telegram.grouping";
pub const TELEGRAM_SOURCE_SCHEMA: &str = "telegram.live_source";
pub const TELEGRAM_RECIPE_SCHEMA: &str = "telegram.remediation_recipe";
pub const TELEGRAM_SCHEMA_VERSION: u16 = 1;

pub fn telegram_provider_key() -> ProviderKey {
    ProviderKey::try_from("telegram".to_owned()).expect("static Telegram provider key is valid")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramEnvironment {
    Production,
    Test,
}

impl TelegramEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramAccountLocator {
    pub environment: TelegramEnvironment,
    pub user_id: String,
}

impl TelegramAccountLocator {
    pub fn new(
        environment: TelegramEnvironment,
        user_id: impl Into<String>,
    ) -> Result<Self, AppError> {
        let locator = Self {
            environment,
            user_id: user_id.into(),
        };
        positive_i64(&locator.user_id)?;
        Ok(locator)
    }

    pub fn into_payload(self) -> VersionedPayload {
        VersionedPayload {
            schema: TELEGRAM_ACCOUNT_SCHEMA.into(),
            version: TELEGRAM_SCHEMA_VERSION,
            payload: serde_json::to_value(self).expect("Telegram account locator serializes"),
        }
    }

    pub fn canonical_identity_key(&self) -> String {
        canonical(&[
            "telegram-account-v1",
            self.environment.as_str(),
            &self.user_id,
        ])
    }

    fn validate(&self) -> Result<(), AppError> {
        positive_i64(&self.user_id).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramConversationLocator {
    pub chat_id: String,
}

impl TelegramConversationLocator {
    pub fn new(chat_id: impl Into<String>) -> Result<Self, AppError> {
        let locator = Self {
            chat_id: chat_id.into(),
        };
        signed_nonzero_i64(&locator.chat_id)?;
        Ok(locator)
    }

    pub fn canonical_key(&self) -> String {
        canonical(&["telegram-conversation-v1", &self.chat_id])
    }

    pub fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Conversation,
            TELEGRAM_CONVERSATION_SCHEMA,
            self.canonical_key(),
            self,
        )
    }

    fn validate(&self) -> Result<(), AppError> {
        signed_nonzero_i64(&self.chat_id).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramMessageLocator {
    pub chat_id: String,
    pub message_id: String,
}

impl TelegramMessageLocator {
    pub fn new(
        chat_id: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Result<Self, AppError> {
        let locator = Self {
            chat_id: chat_id.into(),
            message_id: message_id.into(),
        };
        locator.validate()?;
        Ok(locator)
    }

    pub fn canonical_key(&self) -> String {
        canonical(&["telegram-message-v1", &self.chat_id, &self.message_id])
    }

    pub fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Content,
            TELEGRAM_MESSAGE_SCHEMA,
            self.canonical_key(),
            self,
        )
    }

    pub fn scoped(&self, scope: retract_domain::Scope) -> ScopedResourceRef {
        let resource = self.resource(scope.account_id);
        let id = resource
            .resource_id()
            .expect("validated Telegram message locator derives an ID");
        ScopedResourceRef {
            scope,
            id,
            resource,
        }
    }

    fn validate(&self) -> Result<(), AppError> {
        signed_nonzero_i64(&self.chat_id)?;
        positive_i64(&self.message_id)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelegramActorKind {
    User,
    Chat,
}

impl TelegramActorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Chat => "chat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramActorLocator {
    pub kind: TelegramActorKind,
    pub native_id: String,
}

impl TelegramActorLocator {
    pub fn new(kind: TelegramActorKind, native_id: impl Into<String>) -> Result<Self, AppError> {
        let locator = Self {
            kind,
            native_id: native_id.into(),
        };
        locator.validate()?;
        Ok(locator)
    }

    pub fn canonical_key(&self) -> String {
        canonical(&["telegram-actor-v1", self.kind.as_str(), &self.native_id])
    }

    pub fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Actor,
            TELEGRAM_ACTOR_SCHEMA,
            self.canonical_key(),
            self,
        )
    }

    fn validate(&self) -> Result<(), AppError> {
        match self.kind {
            TelegramActorKind::User => positive_i64(&self.native_id).map(|_| ()),
            TelegramActorKind::Chat => signed_nonzero_i64(&self.native_id).map(|_| ()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramGroupingLocator {
    pub chat_id: String,
    pub grouping_id: String,
}

impl TelegramGroupingLocator {
    pub fn new(
        chat_id: impl Into<String>,
        grouping_id: impl Into<String>,
    ) -> Result<Self, AppError> {
        let locator = Self {
            chat_id: chat_id.into(),
            grouping_id: grouping_id.into(),
        };
        locator.validate()?;
        Ok(locator)
    }

    pub fn canonical_key(&self) -> String {
        canonical(&["telegram-grouping-v1", &self.chat_id, &self.grouping_id])
    }

    pub fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Grouping,
            TELEGRAM_GROUPING_SCHEMA,
            self.canonical_key(),
            self,
        )
    }

    fn validate(&self) -> Result<(), AppError> {
        signed_nonzero_i64(&self.chat_id)?;
        signed_nonzero_i64(&self.grouping_id)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramSourceProfile {}

impl TelegramSourceProfile {
    pub fn payload() -> VersionedPayload {
        VersionedPayload {
            schema: TELEGRAM_SOURCE_SCHEMA.into(),
            version: TELEGRAM_SCHEMA_VERSION,
            payload: serde_json::to_value(Self {}).expect("Telegram source profile serializes"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramRecipeStep {
    pub action: ActionKind,
    pub targets: Vec<ScopedResourceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramRemediationRecipe {
    pub steps: Vec<TelegramRecipeStep>,
}

impl TelegramRemediationRecipe {
    pub fn for_steps(steps: &[ActionStep]) -> Result<VersionedPayload, AppError> {
        let recipe = Self {
            steps: steps
                .iter()
                .map(|step| TelegramRecipeStep {
                    action: step.descriptor.kind,
                    targets: step.targets.clone(),
                })
                .collect(),
        };
        Ok(VersionedPayload {
            schema: TELEGRAM_RECIPE_SCHEMA.into(),
            version: TELEGRAM_SCHEMA_VERSION,
            payload: serde_json::to_value(recipe).map_err(|_| invalid_payload())?,
        })
    }
}

#[derive(Debug, Default)]
pub struct TelegramPayloadValidator;

impl ProviderPayloadValidator for TelegramPayloadValidator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        // This is a compatibility identity, not a display version. Change it
        // whenever any accepted schema, range, canonical key, or recipe
        // agreement rule in this validator changes.
        ProviderValidationPolicyKey::try_from(
            "telegram-payload-policy-6:typed-compatibility-transport-retry-diagnostics-v1"
                .to_owned(),
        )
        .expect("static Telegram validation policy key is valid")
    }

    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        account.validate().map_err(|_| invalid_payload())?;
        if account.provider != telegram_provider_key()
            || account.avatar.is_some()
            || account.native_identity.schema != TELEGRAM_ACCOUNT_SCHEMA
            || account.native_identity.version != TELEGRAM_SCHEMA_VERSION
        {
            return Err(invalid_payload());
        }
        let locator: TelegramAccountLocator =
            serde_json::from_value(account.native_identity.payload.clone())
                .map_err(|_| invalid_payload())?;
        locator.validate()?;
        VerifiedNativeAccountIdentity::try_from(locator.canonical_identity_key())
    }

    fn validate_source(
        &self,
        source: &SourceRecord,
        account: &AccountRecord,
    ) -> Result<(), AppError> {
        source.validate(account).map_err(|_| invalid_payload())?;
        if source.kind != SourceKind::LiveConnection
            || source.provider != telegram_provider_key()
            || source.account_id != account.id
            || source.schema_profile.schema != TELEGRAM_SOURCE_SCHEMA
            || source.schema_profile.version != TELEGRAM_SCHEMA_VERSION
        {
            return Err(invalid_payload());
        }
        serde_json::from_value::<TelegramSourceProfile>(source.schema_profile.payload.clone())
            .map_err(|_| invalid_payload())?;
        self.validate_account(account)?;
        Ok(())
    }

    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        resource.validate().map_err(|_| invalid_payload())?;
        if resource.provider != telegram_provider_key()
            || resource.locator_version != TELEGRAM_SCHEMA_VERSION
        {
            return Err(invalid_payload());
        }
        let expected_key = match (resource.resource_kind, resource.locator_schema.as_str()) {
            (ResourceKind::Conversation, TELEGRAM_CONVERSATION_SCHEMA) => {
                let locator: TelegramConversationLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid_payload())?;
                locator.validate()?;
                locator.canonical_key()
            }
            (ResourceKind::Content, TELEGRAM_MESSAGE_SCHEMA) => {
                let locator: TelegramMessageLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid_payload())?;
                locator.validate()?;
                locator.canonical_key()
            }
            (ResourceKind::Actor, TELEGRAM_ACTOR_SCHEMA) => {
                let locator: TelegramActorLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid_payload())?;
                locator.validate()?;
                locator.canonical_key()
            }
            (ResourceKind::Grouping, TELEGRAM_GROUPING_SCHEMA) => {
                let locator: TelegramGroupingLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid_payload())?;
                locator.validate()?;
                locator.canonical_key()
            }
            _ => return Err(invalid_payload()),
        };
        if resource.canonical_key != expected_key {
            return Err(invalid_payload());
        }
        Ok(())
    }

    fn validate_recipe(&self, plan: &RemediationPlan) -> Result<(), AppError> {
        if plan.recipe.schema == super::recipe::EXECUTION_SCHEMA {
            return super::recipe::TelegramExecutionRecipe::validate_envelope(plan).map(|_| ());
        }
        if plan.recipe.schema != TELEGRAM_RECIPE_SCHEMA
            || plan.recipe.version != TELEGRAM_SCHEMA_VERSION
        {
            return Err(invalid_payload());
        }
        let recipe: TelegramRemediationRecipe =
            serde_json::from_value(plan.recipe.payload.clone()).map_err(|_| invalid_payload())?;
        if recipe.steps.len() != plan.steps.len() {
            return Err(invalid_payload());
        }
        for (recipe_step, plan_step) in recipe.steps.iter().zip(&plan.steps) {
            if recipe_step.action != plan_step.descriptor.kind
                || recipe_step.targets != plan_step.targets
            {
                return Err(invalid_payload());
            }
            for target in &recipe_step.targets {
                target
                    .validate(&plan.scope)
                    .map_err(|_| invalid_payload())?;
                self.validate_resource(&target.resource)?;
            }
        }
        Ok(())
    }

    fn validate_job(
        &self,
        plan: &RemediationPlan,
        job: &retract_domain::ScopedJobRecord,
    ) -> Result<(), AppError> {
        let legacy = super::recipe::TelegramExecutionRecipe::validate_envelope(plan)?;
        let initial = crate::model::JobRecord::new(&legacy);
        let expected = super::normalize::normalize_job(
            &plan.scope,
            &legacy,
            &initial,
            job.started_authorized,
        )?;
        let mut actual_dirty = job.dirty_refs.clone();
        actual_dirty.sort_by_key(|r| r.id);
        let mut expected_dirty = expected.dirty_refs;
        expected_dirty.sort_by_key(|r| r.id);
        if actual_dirty != expected_dirty
            || job.counters.selected != legacy.summary.selected as u64
            || job.counters.eligible != legacy.summary.delete_for_everyone as u64
        {
            return Err(invalid_payload());
        }
        let batches = legacy.everyone_batches(100)?;
        let max_cursor = if legacy.operation == cleaner_domain::PlanOperation::ClearHistoryAndLeave
        {
            1
        } else {
            batches.len()
        };
        if job.next_batch > max_cursor as u64 {
            return Err(invalid_payload());
        }
        let processed = batches
            .iter()
            .take(job.next_batch as usize)
            .map(|b| b.message_ids.len() as u64)
            .sum::<u64>();
        let initial_skipped = (legacy.summary.self_only + legacy.summary.cannot_delete) as u64;
        if job.counters.skipped < initial_skipped {
            return Err(invalid_payload());
        }
        let accounted = job
            .counters
            .deleted
            .checked_add(job.counters.failed)
            .and_then(|n| n.checked_add(job.counters.uncertain))
            .and_then(|n| n.checked_add(job.counters.skipped - initial_skipped))
            .ok_or_else(invalid_payload)?;
        if processed != accounted {
            return Err(invalid_payload());
        }
        Ok(())
    }
}

fn resource(
    account_id: AccountId,
    resource_kind: ResourceKind,
    locator_schema: &str,
    canonical_key: String,
    locator: &impl Serialize,
) -> ProviderResourceRef {
    ProviderResourceRef {
        provider: telegram_provider_key(),
        account_id,
        resource_kind,
        locator_schema: locator_schema.into(),
        locator_version: TELEGRAM_SCHEMA_VERSION,
        canonical_key,
        locator_payload: serde_json::to_value(locator).expect("Telegram locator serializes"),
    }
}

fn canonical(parts: &[&str]) -> String {
    serde_json::to_string(parts).expect("static Telegram canonical tuple serializes")
}

fn positive_i64(value: &str) -> Result<i64, AppError> {
    let parsed = canonical_i64(value)?;
    if parsed <= 0 {
        return Err(invalid_payload());
    }
    Ok(parsed)
}

fn signed_nonzero_i64(value: &str) -> Result<i64, AppError> {
    let parsed = canonical_i64(value)?;
    if parsed == 0 {
        return Err(invalid_payload());
    }
    Ok(parsed)
}

fn canonical_i64(value: &str) -> Result<i64, AppError> {
    let parsed = value.parse::<i64>().map_err(|_| invalid_payload())?;
    if parsed.to_string() != value {
        return Err(invalid_payload());
    }
    Ok(parsed)
}

fn invalid_payload() -> AppError {
    AppError::SecureStore("invalid Telegram provider payload".into())
}
