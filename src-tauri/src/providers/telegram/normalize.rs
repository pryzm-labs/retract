use cleaner_domain::{DeletionPlan, DeletionReach};
use retract_domain::{
    ActionDescriptor, ActionKind, Availability, BatchConstraints, ConfirmationTier, ErrorCode,
    ExpectedEffect, JobCounters, Scope, ScopedJobRecord, ScopedResourceRef, VersionedPayload,
};
use serde::{Deserialize, Serialize};

use super::{
    diagnostics::{invalid_recipe, legacy_diagnostic_code, safe_diagnostic},
    locators::{
        TelegramActorKind, TelegramActorLocator, TelegramConversationLocator,
        TelegramMessageLocator,
    },
};
use crate::{
    error::AppError,
    model::{JobRecord, JobStatus},
};

pub(crate) fn descriptor(
    kind: ActionKind,
    effect: ExpectedEffect,
    tier: ConfirmationTier,
) -> ActionDescriptor {
    ActionDescriptor {
        id: format!("telegram.{kind:?}"),
        kind,
        effect,
        availability: Availability::LivePreflightRequired,
        unavailable_reason: None,
        requires_live_preflight: true,
        batch: BatchConstraints {
            max_targets: if kind == ActionKind::DeleteRemoteItem {
                100
            } else {
                1
            },
            max_parallel: 1,
        },
        confirmation_tier: tier,
        destructive: true,
        irreversible: true,
        advisory: Some(retract_domain::ActionAdvisory {
            cost_bearing: false,
            rate_limited: true,
        }),
    }
}

pub(crate) fn conversation_ref(scope: &Scope, id: i64) -> Result<ScopedResourceRef, AppError> {
    let resource = TelegramConversationLocator::new(id.to_string())?.resource(scope.account_id);
    Ok(ScopedResourceRef {
        scope: scope.clone(),
        id: resource.resource_id().map_err(|_| invalid_recipe())?,
        resource,
    })
}

pub(crate) fn actor_locator(id: i64) -> Result<TelegramActorLocator, AppError> {
    TelegramActorLocator::new(
        if id > 0 {
            TelegramActorKind::User
        } else {
            TelegramActorKind::Chat
        },
        id.to_string(),
    )
}

pub fn normalize_job(
    scope: &Scope,
    plan: &DeletionPlan,
    job: &JobRecord,
    started_authorized: bool,
) -> Result<ScopedJobRecord, AppError> {
    let status = match job.status {
        JobStatus::Queued => retract_domain::JobStatus::Queued,
        JobStatus::Running => retract_domain::JobStatus::Running,
        JobStatus::Completed => retract_domain::JobStatus::Completed,
        JobStatus::Partial => retract_domain::JobStatus::Partial,
        JobStatus::Failed => retract_domain::JobStatus::Failed,
        JobStatus::Cancelled => retract_domain::JobStatus::Cancelled,
    };
    Ok(ScopedJobRecord {
        id: job.id,
        plan_id: job.plan_id,
        scope: scope.clone(),
        dirty_refs: job
            .target_chat_ids
            .iter()
            .map(|id| conversation_ref(scope, *id))
            .collect::<Result<_, _>>()?,
        status,
        counters: JobCounters {
            selected: plan.summary.selected as u64,
            eligible: job.total as u64,
            deleted: job.deleted as u64,
            skipped: job.skipped as u64,
            failed: job.failed as u64,
            uncertain: job.uncertain as u64,
        },
        next_batch: job.next_batch as u64,
        retry_at: job.retry_at.or_else(|| {
            job.retry_after_seconds
                .map(|s| job.updated_at + chrono::Duration::seconds(s.min(86400) as i64))
        }),
        diagnostics: job
            .error_codes
            .iter()
            .enumerate()
            .map(|(index, code)| {
                job.scoped_diagnostics
                    .get(index)
                    .filter(|d| legacy_diagnostic_code(d.code) == code)
                    .cloned()
                    .unwrap_or_else(|| {
                        let mut diagnostic = safe_diagnostic(code);
                        if diagnostic.code == ErrorCode::RateLimited {
                            diagnostic.retry_at = job.retry_at;
                        }
                        diagnostic
                    })
            })
            .collect(),
        started_authorized,
        created_at: job.created_at,
        updated_at: job.updated_at,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramContentMetadata {
    pub original_kind: cleaner_domain::ContentKind,
    pub outgoing: bool,
    pub pinned: bool,
    pub grouping: Option<ScopedResourceRef>,
    pub sender_name: String,
    pub actor: Option<ScopedResourceRef>,
    pub deletion_reach: DeletionReach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramConversationMetadata {
    pub original_kind: cleaner_domain::ChatKind,
    pub archived: bool,
    pub conversation_state: cleaner_domain::ConversationState,
    pub capabilities: cleaner_domain::ChatCapabilities,
    pub avatar_seed: u8,
}

fn metadata(schema: &str, value: &impl Serialize) -> Result<VersionedPayload, AppError> {
    Ok(VersionedPayload {
        schema: schema.into(),
        version: 1,
        payload: serde_json::to_value(value).map_err(|_| invalid_recipe())?,
    })
}

pub fn normalize_conversation(
    scope: &Scope,
    chat: &cleaner_domain::ChatSummary,
) -> Result<retract_domain::ConversationRecord, AppError> {
    let resource =
        TelegramConversationLocator::new(chat.id.to_string())?.resource(scope.account_id);
    let record = retract_domain::ConversationRecord {
        id: resource
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        scope: scope.clone(),
        resource,
        kind: match chat.kind {
            cleaner_domain::ChatKind::Direct | cleaner_domain::ChatKind::Secret => {
                retract_domain::ConversationKind::Direct
            }
            cleaner_domain::ChatKind::BasicGroup | cleaner_domain::ChatKind::Supergroup => {
                retract_domain::ConversationKind::Group
            }
            cleaner_domain::ChatKind::Channel => retract_domain::ConversationKind::Broadcast,
        },
        title: chat.title.clone(),
        parent_id: None,
        participant_count: chat.member_count.map(u64::from),
        participants: Vec::new(),
        evidence: retract_domain::EvidenceState::Live,
        observed_at: chrono::Utc::now(),
        provider_metadata: Some(metadata(
            "telegram.conversation_metadata",
            &TelegramConversationMetadata {
                original_kind: chat.kind,
                archived: chat.archived,
                conversation_state: chat.conversation_state,
                capabilities: chat.capabilities.clone(),
                avatar_seed: chat.avatar_seed,
            },
        )?),
    };
    record.validate(scope).map_err(|_| invalid_recipe())?;
    Ok(record)
}

pub fn normalize_content(
    scope: &Scope,
    message: &cleaner_domain::MessageSnapshot,
) -> Result<retract_domain::ContentRecord, AppError> {
    let resource =
        TelegramMessageLocator::new(message.chat_id.to_string(), message.message_id.to_string())?
            .resource(scope.account_id);
    let actor = actor_locator(message.sender_id)?.resource(scope.account_id);
    let actor_ref = ScopedResourceRef {
        scope: scope.clone(),
        id: actor.resource_id().map_err(|_| invalid_recipe())?,
        resource: actor.clone(),
    };
    actor_ref.validate(scope).map_err(|_| invalid_recipe())?;
    let grouping = message
        .album_id
        .map(|id| -> Result<ScopedResourceRef, AppError> {
            let resource = super::locators::TelegramGroupingLocator::new(
                message.chat_id.to_string(),
                id.to_string(),
            )?
            .resource(scope.account_id);
            Ok(ScopedResourceRef {
                scope: scope.clone(),
                id: resource.resource_id().map_err(|_| invalid_recipe())?,
                resource,
            })
        })
        .transpose()?;
    let kind = match message.content_kind {
        cleaner_domain::ContentKind::Photo => retract_domain::ContentKind::Image,
        cleaner_domain::ContentKind::File => retract_domain::ContentKind::Document,
        other => serde_json::from_value(serde_json::to_value(other).map_err(|_| invalid_recipe())?)
            .map_err(|_| invalid_recipe())?,
    };
    let privacy_findings = message
        .privacy_findings
        .iter()
        .map(|finding| {
            serde_json::from_value(serde_json::to_value(finding).map_err(|_| invalid_recipe())?)
                .map_err(|_| invalid_recipe())
        })
        .collect::<Result<_, _>>()?;
    let record = retract_domain::ContentRecord {
        id: resource
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        scope: scope.clone(),
        conversation_id: conversation_ref(scope, message.chat_id)?
            .id
            .try_into()
            .map_err(|_| invalid_recipe())?,
        resource,
        author_id: actor
            .resource_id()
            .map_err(|_| invalid_recipe())?
            .try_into()
            .map_err(|_| invalid_recipe())?,
        timestamp: message.sent_at,
        edited_at: None,
        kind,
        searchable_text: message.preview.clone(),
        attachments: Vec::new(),
        reply_to: None,
        thread_parent: None,
        external_location: retract_domain::ExternalLocationAvailability::Unsupported,
        evidence: retract_domain::EvidenceState::Live,
        observed_at: chrono::Utc::now(),
        privacy_findings,
        detector_version: if message.privacy_findings.is_empty() {
            None
        } else {
            Some("telegram-sensitive-v1".into())
        },
        provider_metadata: Some(metadata(
            "telegram.content_metadata",
            &TelegramContentMetadata {
                original_kind: message.content_kind,
                outgoing: message.is_outgoing,
                pinned: message.is_pinned,
                grouping,
                sender_name: message.sender_name.clone(),
                actor: Some(actor_ref),
                deletion_reach: message.deletion_reach,
            },
        )?),
    };
    record.validate(scope).map_err(|_| invalid_recipe())?;
    Ok(record)
}
