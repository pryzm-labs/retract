use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use retract_domain::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::http::{
    DeleteOutcome, DiscordDeleteClient, DiscordDeleteError, DiscordLiveMessageBinding,
};
use super::locators::{
    DiscordMessageLocator, DiscordPayloadValidator, MESSAGE_SCHEMA, canonical_id,
};
use super::model::{CONTENT_METADATA_SCHEMA, DiscordContentMetadata, decode};
use super::session::DiscordSessionOwner;
use crate::persistence::ProviderPayloadValidator;
use crate::providers::frozen_lifecycle::FrozenProviderIo;
use crate::providers::ports::{IntentDescriptor, PrepareIntent, QuerySource, ResolveRequest};

pub(crate) const DELETE_RECIPE_SCHEMA: &str = "discord.delete_messages.v1";
const DELETE_ACTION_ID: &str = "discord.delete_messages.v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscordDeleteRecipe {
    owner_user_id: String,
    target_count: u32,
    minimum_channel_delay_ms: u32,
}

#[async_trait]
pub(crate) trait DiscordMessageDelete: Send + Sync {
    async fn verify_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError>;

    async fn delete_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<DeleteOutcome, DiscordDeleteError>;
}

#[async_trait]
impl DiscordMessageDelete for DiscordDeleteClient {
    async fn verify_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        self.verify_owned(
            channel_id,
            message_id,
            owner_user_id,
            expected,
            token,
            cancelled,
        )
        .await
    }

    async fn delete_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<DeleteOutcome, DiscordDeleteError> {
        self.delete_owned(
            channel_id,
            message_id,
            owner_user_id,
            expected,
            token,
            cancelled,
        )
        .await
    }
}

pub(crate) struct DiscordRemediationIo {
    context: ActiveContext,
    owner_user_id: String,
    query: Arc<dyn QuerySource>,
    session: Arc<DiscordSessionOwner>,
    delete: Arc<dyn DiscordMessageDelete>,
}

impl DiscordRemediationIo {
    pub(crate) fn production(
        context: ActiveContext,
        owner_user_id: String,
        query: Arc<dyn QuerySource>,
        session: Arc<DiscordSessionOwner>,
    ) -> Result<Self, SafeError> {
        let delete =
            Arc::new(DiscordDeleteClient::production().map_err(|_| safe(ErrorCode::Transient))?);
        Self::new(context, owner_user_id, query, session, delete)
    }

    pub(crate) fn new(
        context: ActiveContext,
        owner_user_id: String,
        query: Arc<dyn QuerySource>,
        session: Arc<DiscordSessionOwner>,
        delete: Arc<dyn DiscordMessageDelete>,
    ) -> Result<Self, SafeError> {
        context
            .validate()
            .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        if context.scope.provider.as_str() != "discord" || canonical_id(&owner_user_id).is_err() {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        Ok(Self {
            context,
            owner_user_id,
            query,
            session,
            delete,
        })
    }

    fn check_scope(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != &self.context {
            return Err(safe(ErrorCode::StaleContext));
        }
        Ok(())
    }

    fn check_context(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check_scope(context)?;
        self.session
            .with_token(&self.owner_user_id, |_| ())
            .map_err(|_| safe(ErrorCode::AuthenticationRequired))
    }

    async fn resolve_owned(
        &self,
        targets: &[ScopedResourceRef],
    ) -> Result<Vec<ContentRecord>, SafeError> {
        if targets.is_empty() || targets.len() > 100_000 {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        for target in targets {
            target
                .validate(&self.context.scope)
                .map_err(|_| safe(ErrorCode::ScopeMismatch))?;
            DiscordPayloadValidator
                .validate_resource(&target.resource)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
            if target.resource.resource_kind != ResourceKind::Content
                || target.resource.locator_schema != MESSAGE_SCHEMA
            {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
        }
        let records = self
            .query
            .resolve(ResolveRequest {
                scope: self.context.scope.clone(),
                refs: targets.to_vec(),
            })
            .await
            .map_err(provider_error)?;
        if records.len() != targets.len() {
            return Err(safe(ErrorCode::NotFound));
        }
        for target in targets {
            let record = records
                .iter()
                .find(|record| *record.id.as_uuid() == target.id)
                .ok_or_else(|| safe(ErrorCode::NotFound))?;
            DiscordPayloadValidator
                .validate_archive_content(record)
                .map_err(|_| safe(ErrorCode::InvalidArchive))?;
            if record.resource != target.resource {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let metadata: DiscordContentMetadata = decode(
                record
                    .provider_metadata
                    .as_ref()
                    .ok_or_else(|| safe(ErrorCode::InvalidArchive))?,
                CONTENT_METADATA_SCHEMA,
            )
            .map_err(|_| safe(ErrorCode::InvalidArchive))?;
            if metadata.author_user_id != self.owner_user_id {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
        }
        Ok(records)
    }

    fn token(&self) -> Result<Zeroizing<String>, SafeError> {
        self.session
            .with_token(&self.owner_user_id, |value| {
                Zeroizing::new(value.to_owned())
            })
            .map_err(|_| safe(ErrorCode::AuthenticationRequired))
    }

    async fn verify_live_owned(
        &self,
        records: &[ContentRecord],
        cancelled: &AtomicBool,
    ) -> Result<(), SafeError> {
        let token = self.token()?;
        for record in records {
            let locator: DiscordMessageLocator =
                serde_json::from_value(record.resource.locator_payload.clone())
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
            let attachment_names = record
                .attachments
                .iter()
                .map(|attachment| attachment.safe_display_name.as_deref())
                .collect::<Vec<_>>();
            let expected = DiscordLiveMessageBinding::with_attachment_names(
                &record.searchable_text,
                record.timestamp.timestamp_millis(),
                &attachment_names,
            );
            self.delete
                .verify_owned(
                    &locator.channel_id,
                    &locator.message_id,
                    &self.owner_user_id,
                    &expected,
                    &token,
                    cancelled,
                )
                .await
                .map_err(|error| self.map_delete_error(error))?;
        }
        Ok(())
    }

    fn map_delete_error(&self, error: DiscordDeleteError) -> SafeError {
        match error {
            DiscordDeleteError::Authentication => {
                self.session.shutdown();
                safe(ErrorCode::AuthenticationRequired)
            }
            DiscordDeleteError::Permission | DiscordDeleteError::OwnershipMismatch => {
                safe(ErrorCode::PermissionChanged)
            }
            DiscordDeleteError::NotFound => safe(ErrorCode::NotFound),
            DiscordDeleteError::RateLimited {
                retry_after_millis, ..
            } => SafeError {
                code: ErrorCode::RateLimited,
                retry_at: Some(
                    Utc::now()
                        + ChronoDuration::milliseconds(
                            i64::try_from(retry_after_millis).unwrap_or(i64::MAX),
                        ),
                ),
            },
            DiscordDeleteError::Transient => safe(ErrorCode::Transient),
            DiscordDeleteError::Ambiguous => safe(ErrorCode::AmbiguousOutcome),
            DiscordDeleteError::Permanent => safe(ErrorCode::Permanent),
            DiscordDeleteError::InvalidTarget => safe(ErrorCode::UnsupportedSchema),
            DiscordDeleteError::Cancelled => safe(ErrorCode::StaleContext),
        }
    }
}

#[async_trait]
impl FrozenProviderIo for DiscordRemediationIo {
    fn check(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check_scope(context)
    }

    async fn describe(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
        id: Uuid,
    ) -> Result<RemediationPlan, SafeError> {
        self.check_context(context)?;
        if intent.action_id != "selected_messages" || intent.actor.is_some() {
            return Err(safe(ErrorCode::UnsupportedSchema));
        }
        let records = self.resolve_owned(&intent.targets).await?;
        self.verify_live_owned(&records, &AtomicBool::new(false))
            .await?;
        self.check_context(context)?;
        let descriptor = descriptor();
        let recipe = DiscordDeleteRecipe {
            owner_user_id: self.owner_user_id.clone(),
            target_count: u32::try_from(intent.targets.len())
                .map_err(|_| safe(ErrorCode::ScopeMismatch))?,
            minimum_channel_delay_ms: 1500,
        };
        let mut plan = RemediationPlan {
            id,
            scope: context.scope.clone(),
            steps: vec![ActionStep {
                descriptor,
                targets: intent.targets.clone(),
            }],
            targets: intent.targets,
            confirmation: ConfirmationRequirements {
                tier: ConfirmationTier::High,
                acknowledgement_required: true,
                owner_auth_required: true,
                exact_text: Some("DELETE DISCORD MESSAGES".into()),
            },
            recipe: VersionedPayload {
                schema: DELETE_RECIPE_SCHEMA.into(),
                version: 1,
                payload: serde_json::to_value(recipe)
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
            },
            restart_policy: RestartPolicy::RequiresNewReview,
            created_at: Utc::now(),
            fingerprint: String::new(),
        };
        plan.seal().map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        validate_plan(&plan).map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        Ok(plan)
    }

    fn dirty_refs(&self, plan: &RemediationPlan) -> Result<Vec<ScopedResourceRef>, SafeError> {
        validate_plan(plan).map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        Ok(plan.targets.clone())
    }

    async fn owner_prompt(&self, plan: &RemediationPlan) -> Result<(), SafeError> {
        let reason = authorization_reason(plan).map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        crate::local_auth::authenticate(&reason)
            .await
            .map_err(|_| safe(ErrorCode::AuthenticationRequired))
    }

    async fn preflight(
        &self,
        target: &ScopedResourceRef,
        cancelled: &AtomicBool,
    ) -> Result<bool, SafeError> {
        self.check_context(&self.context)?;
        let records = self.resolve_owned(std::slice::from_ref(target)).await?;
        self.verify_live_owned(&records, cancelled).await?;
        self.check_context(&self.context)?;
        Ok(true)
    }

    async fn mutate(
        &self,
        plan: &RemediationPlan,
        targets: &[ScopedResourceRef],
        cancelled: &AtomicBool,
    ) -> Result<(), SafeError> {
        self.check_context(&self.context)?;
        validate_plan(plan).map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        let recipe: DiscordDeleteRecipe = serde_json::from_value(plan.recipe.payload.clone())
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        if recipe.owner_user_id != self.owner_user_id {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        if targets.len() != 1 || !plan.targets.contains(&targets[0]) {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        let records = self.resolve_owned(targets).await?;
        self.check_context(&self.context)?;
        let locator: DiscordMessageLocator =
            serde_json::from_value(targets[0].resource.locator_payload.clone())
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
        let token = self.token()?;
        let attachment_names = records[0]
            .attachments
            .iter()
            .map(|attachment| attachment.safe_display_name.as_deref())
            .collect::<Vec<_>>();
        let expected = DiscordLiveMessageBinding::with_attachment_names(
            &records[0].searchable_text,
            records[0].timestamp.timestamp_millis(),
            &attachment_names,
        );
        if cancelled.load(Ordering::Acquire) {
            return Err(safe(ErrorCode::StaleContext));
        }
        match self
            .delete
            .delete_owned(
                &locator.channel_id,
                &locator.message_id,
                &self.owner_user_id,
                &expected,
                &token,
                cancelled,
            )
            .await
        {
            Ok(DeleteOutcome::Deleted | DeleteOutcome::AlreadyAbsent) => Ok(()),
            Err(error) => Err(self.map_delete_error(error)),
        }
    }

    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        self.check_context(context)?;
        self.resolve_owned(&targets).await?;
        self.check_context(context)?;
        Ok(vec![IntentDescriptor {
            action_id: "selected_messages".into(),
            label: "Delete selected Discord messages".into(),
            requires_actor: false,
            descriptors: vec![descriptor()],
        }])
    }
}

pub(super) fn authorization_reason(
    plan: &RemediationPlan,
) -> Result<String, crate::error::AppError> {
    use crate::error::AppError;

    validate_plan(plan)?;
    let recipe: DiscordDeleteRecipe = serde_json::from_value(plan.recipe.payload.clone())
        .map_err(|_| AppError::InvalidRequest("invalid Discord deletion plan".into()))?;
    let plan_token = retract_domain::plan_fingerprint_token(&plan.fingerprint)
        .map_err(|_| AppError::InvalidRequest("invalid Discord deletion plan".into()))?;
    let first_target = plan
        .targets
        .first()
        .ok_or_else(|| AppError::InvalidRequest("invalid Discord deletion plan".into()))?;
    let locator: DiscordMessageLocator =
        serde_json::from_value(first_target.resource.locator_payload.clone())
            .map_err(|_| AppError::InvalidRequest("invalid Discord deletion target".into()))?;
    let target_count = recipe.target_count;
    let target_word = if target_count == 1 {
        "message"
    } else {
        "messages"
    };
    let remainder = target_count.saturating_sub(1);
    let remainder = if remainder == 0 {
        String::new()
    } else {
        format!(" (+{remainder} more)")
    };
    let reason = format!(
        "Discord plan {plan_token}: delete {target_count} reviewed {target_word} owned by account {} from source {}; target channel {}/message {}{remainder}.",
        recipe.owner_user_id,
        plan.scope.source_id.as_uuid(),
        locator.channel_id,
        locator.message_id,
    );
    if reason.chars().count() > 256 || reason.chars().any(char::is_control) {
        return Err(AppError::InvalidRequest(
            "the Discord deletion plan cannot be represented safely for authentication".into(),
        ));
    }
    Ok(reason)
}

fn descriptor() -> ActionDescriptor {
    ActionDescriptor {
        id: DELETE_ACTION_ID.into(),
        kind: ActionKind::DeleteRemoteItem,
        effect: ExpectedEffect::RemovedForAllParticipants,
        availability: Availability::LivePreflightRequired,
        unavailable_reason: None,
        requires_live_preflight: true,
        batch: BatchConstraints {
            max_targets: 1,
            max_parallel: 4,
        },
        confirmation_tier: ConfirmationTier::High,
        destructive: true,
        irreversible: true,
        advisory: Some(ActionAdvisory {
            cost_bearing: false,
            rate_limited: true,
        }),
    }
}

pub(crate) fn validate_plan(plan: &RemediationPlan) -> Result<(), crate::error::AppError> {
    use crate::error::AppError;
    plan.validate()
        .map_err(|_| AppError::InvalidRequest("invalid Discord deletion plan".into()))?;
    if plan.scope.provider.as_str() != "discord"
        || plan.recipe.schema != DELETE_RECIPE_SCHEMA
        || plan.recipe.version != 1
        || plan.restart_policy != RestartPolicy::RequiresNewReview
        || plan.steps.len() != 1
        || plan.steps[0].descriptor != descriptor()
        || plan.steps[0].targets != plan.targets
        || plan.confirmation.tier != ConfirmationTier::High
        || plan.confirmation.exact_text.as_deref() != Some("DELETE DISCORD MESSAGES")
    {
        return Err(AppError::InvalidRequest(
            "invalid Discord deletion plan".into(),
        ));
    }
    let recipe: DiscordDeleteRecipe = serde_json::from_value(plan.recipe.payload.clone())
        .map_err(|_| AppError::InvalidRequest("invalid Discord deletion plan".into()))?;
    canonical_id(&recipe.owner_user_id)?;
    if recipe.minimum_channel_delay_ms != 1500
        || usize::try_from(recipe.target_count).ok() != Some(plan.targets.len())
    {
        return Err(AppError::InvalidRequest(
            "invalid Discord deletion plan".into(),
        ));
    }
    for target in &plan.targets {
        target
            .validate(&plan.scope)
            .map_err(|_| AppError::InvalidRequest("invalid Discord deletion target".into()))?;
        DiscordPayloadValidator.validate_resource(&target.resource)?;
        if target.resource.resource_kind != ResourceKind::Content {
            return Err(AppError::InvalidRequest(
                "invalid Discord deletion target".into(),
            ));
        }
    }
    Ok(())
}

fn provider_error(error: ProviderError) -> SafeError {
    let code = match error.code {
        ProviderErrorKind::AuthenticationRequired => ErrorCode::AuthenticationRequired,
        ProviderErrorKind::PermissionChanged => ErrorCode::PermissionChanged,
        ProviderErrorKind::NotFound => ErrorCode::NotFound,
        ProviderErrorKind::AlreadyRemoved => ErrorCode::AlreadyRemoved,
        ProviderErrorKind::RateLimited => ErrorCode::RateLimited,
        ProviderErrorKind::CostLimitReached => ErrorCode::CostLimitReached,
        ProviderErrorKind::Transient => ErrorCode::Transient,
        ProviderErrorKind::Permanent => ErrorCode::Permanent,
        ProviderErrorKind::AmbiguousOutcome => ErrorCode::AmbiguousOutcome,
        ProviderErrorKind::UnsupportedSchema => ErrorCode::UnsupportedSchema,
        ProviderErrorKind::InvalidArchive => ErrorCode::InvalidArchive,
    };
    SafeError {
        code,
        retry_at: error.retry_at,
    }
}

fn safe(code: ErrorCode) -> SafeError {
    SafeError {
        code,
        retry_at: None,
    }
}
