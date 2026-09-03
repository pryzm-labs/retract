//! Production composition and typed application operations for the Telegram bridge.
use super::{
    LiveGateway, TelegramGateway,
    compat::{TelegramCompatibilityProvider, TelegramExecutionRecipe, normalize_conversation},
    engine_context::{EngineContext, FoundationTelegramRepository},
    identity::IdentityVerificationStatus,
    locators::{
        TelegramActorLocator, TelegramConversationLocator, TelegramMessageLocator,
        TelegramPayloadValidator,
    },
};
use crate::{
    compatibility::model_v2 as wire,
    error::boundary_error,
    persistence::{FoundationStore, ProviderPayloadValidator},
    provider_service::{safe, validate_refs},
    providers::{ports::*, registry::ProviderRegistryError},
    service::CleanerService,
};
use async_trait::async_trait;
use retract_domain::{ActiveContext, ErrorCode, ResourceKind, SafeError, ScopedResourceRef};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

impl TelegramCompatibilityProvider {
    fn check_active(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if context != self.context.active() {
            return Err(safe(ErrorCode::StaleContext));
        }
        self.context
            .check(self.gateway.as_ref())
            .map_err(boundary_error)
    }
    fn normalize_live_job(
        &self,
        job: &crate::model::JobRecord,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        let envelope = self
            .engine
            .reviewed_plan(job.plan_id)
            .map_err(boundary_error)?;
        let legacy =
            TelegramExecutionRecipe::validate_envelope(&envelope).map_err(boundary_error)?;
        Self::normalize_job(&envelope.scope, &legacy, job, true).map_err(boundary_error)
    }
}

impl ProviderRegistration for TelegramCompatibilityProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            key: super::locators::telegram_provider_key(),
            display_name: "Telegram".into(),
            capabilities: [
                ProviderCapability::ConversationListing,
                ProviderCapability::ContentSearch,
                ProviderCapability::MediaMetadata,
            ]
            .into_iter()
            .collect(),
        }
    }
    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        Ok(Arc::new(self.clone()))
    }
    fn application_query(&self) -> Result<Arc<dyn ApplicationQuery>, ProviderRegistryError> {
        Ok(Arc::new(self.clone()))
    }
    fn reviewed_lifecycle(&self) -> Result<Arc<dyn ReviewedLifecycle>, ProviderRegistryError> {
        Ok(Arc::new(self.clone()))
    }
    fn payload_validator(
        &self,
    ) -> Result<Arc<dyn ProviderPayloadValidator>, ProviderRegistryError> {
        Ok(Arc::new(TelegramPayloadValidator))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelegramSearchFilters {
    #[serde(default)]
    pub chat_kinds: Vec<cleaner_domain::ChatKind>,
    #[serde(default)]
    pub content_kinds: Vec<cleaner_domain::ContentKind>,
    #[serde(default)]
    pub direction: crate::model::MessageDirection,
    pub min_date: Option<chrono::DateTime<chrono::Utc>>,
    pub max_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub exclude_pinned: bool,
    #[serde(default)]
    pub privacy_scan: bool,
}

fn chat_id(reference: &ScopedResourceRef) -> Result<i64, SafeError> {
    if reference.resource.resource_kind != ResourceKind::Conversation {
        return Err(safe(ErrorCode::ScopeMismatch));
    }
    let locator: TelegramConversationLocator =
        serde_json::from_value(reference.resource.locator_payload.clone())
            .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
    locator
        .chat_id
        .parse()
        .map_err(|_| safe(ErrorCode::UnsupportedSchema))
}

#[async_trait]
impl ApplicationQuery for TelegramCompatibilityProvider {
    async fn conversations(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<retract_domain::ConversationRecord>, SafeError> {
        self.check_active(context)?;
        let chats = self.gateway.chats().await.map_err(boundary_error)?;
        self.check_active(context)?;
        chats
            .iter()
            .map(|c| normalize_conversation(&context.scope, c).map_err(boundary_error))
            .collect()
    }
    async fn search_filtered(
        &self,
        context: &ActiveContext,
        request: wire::SearchRequest,
    ) -> Result<Page<retract_domain::ContentRecord>, SafeError> {
        self.check_active(context)?;
        validate_refs(
            context,
            &request.conversations,
            &TelegramPayloadValidator,
            Some(ResourceKind::Conversation),
        )?;
        let filters = if let Some(filters) = request.filters {
            if filters.schema != "telegram.search_filters" || filters.version != 1 {
                return Err(safe(ErrorCode::UnsupportedSchema));
            }
            serde_json::from_value::<TelegramSearchFilters>(filters.payload)
                .map_err(|_| safe(ErrorCode::UnsupportedSchema))?
        } else {
            TelegramSearchFilters::default()
        };
        let items = self
            .search_filtered(crate::model::SearchRequest {
                query: request.query,
                chat_ids: request
                    .conversations
                    .iter()
                    .map(chat_id)
                    .collect::<Result<_, _>>()?,
                chat_kinds: filters.chat_kinds,
                content_kinds: filters.content_kinds,
                direction: filters.direction,
                min_date: filters.min_date,
                max_date: filters.max_date,
                exclude_pinned: filters.exclude_pinned,
                privacy_scan: filters.privacy_scan,
                limit: request.limit as usize,
            })
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        Ok(Page {
            items,
            next_cursor: None,
        })
    }
    async fn refresh(
        &self,
        context: &ActiveContext,
        refs: Vec<ScopedResourceRef>,
    ) -> Result<Vec<retract_domain::ConversationRecord>, SafeError> {
        self.check_active(context)?;
        validate_refs(
            context,
            &refs,
            &TelegramPayloadValidator,
            Some(ResourceKind::Conversation),
        )?;
        let chats = self
            .engine
            .refresh_chats(refs.iter().map(chat_id).collect::<Result<_, _>>()?)
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        chats
            .iter()
            .map(|c| normalize_conversation(&context.scope, c).map_err(boundary_error))
            .collect()
    }
}

#[async_trait]
impl ReviewedLifecycle for TelegramCompatibilityProvider {
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        self.check_active(context)?;
        validate_refs(context, &targets, &TelegramPayloadValidator, None)?;
        if targets.is_empty()
            || (targets
                .iter()
                .any(|t| t.resource.resource_kind != ResourceKind::Content)
                && (targets.len() != 1
                    || targets[0].resource.resource_kind != ResourceKind::Conversation))
        {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        let actions = self
            .actions_for(ActionRequest {
                context: context.clone(),
                targets: targets.clone(),
            })
            .await
            .map_err(boundary_error)?;
        let descriptors = actions
            .into_iter()
            .flat_map(|a| a.descriptors)
            .collect::<Vec<_>>();
        let ids: &[(&str, &str, bool)] = if targets
            .iter()
            .all(|t| t.resource.resource_kind == ResourceKind::Content)
        {
            &[(
                "selected_messages",
                "Delete selected messages for everyone",
                false,
            )]
        } else {
            &[
                (
                    "delete_my_messages",
                    "Delete my messages for everyone",
                    false,
                ),
                ("clear_history", "Clear history for everyone", false),
                ("remove_chat_for_self", "Remove chat for me", false),
                ("leave_chat", "Clean up and leave chat", false),
                ("delete_by_sender", "Delete messages by sender", true),
                ("delete_group", "Permanently delete group", false),
            ]
        };
        self.check_active(context)?;
        Ok(ids
            .iter()
            .map(|(id, label, actor)| {
                use retract_domain::{ActionKind, ConfirmationTier, ExpectedEffect};
                let kind = match *id {
                    "selected_messages" | "delete_my_messages" => ActionKind::DeleteRemoteItem,
                    "clear_history" => ActionKind::ClearConversation,
                    "remove_chat_for_self" => ActionKind::RemoveForCurrentAccount,
                    "leave_chat" => ActionKind::LeaveConversation,
                    "delete_by_sender" => ActionKind::DeleteByActor,
                    "delete_group" => ActionKind::DeleteConversation,
                    _ => unreachable!("closed intent catalog"),
                };
                let descriptors = if *id == "delete_my_messages" {
                    vec![super::compat::descriptor(
                        kind,
                        ExpectedEffect::RemovedForAllParticipants,
                        ConfirmationTier::High,
                    )]
                } else {
                    descriptors
                        .iter()
                        .filter(|d| d.kind == kind)
                        .cloned()
                        .collect()
                };
                IntentDescriptor {
                    action_id: (*id).into(),
                    label: (*label).into(),
                    requires_actor: *actor,
                    descriptors,
                }
            })
            .collect())
    }
    async fn prepare(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
    ) -> Result<retract_domain::RemediationPlan, SafeError> {
        self.check_active(context)?;
        validate_refs(context, &intent.targets, &TelegramPayloadValidator, None)?;
        let view = if intent.action_id == "selected_messages" {
            if intent.actor.is_some() {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let mut refs = Vec::new();
            for reference in &intent.targets {
                if reference.resource.resource_kind != ResourceKind::Content {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let locator: TelegramMessageLocator =
                    serde_json::from_value(reference.resource.locator_payload.clone())
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                refs.push(crate::model::MessageRef {
                    chat_id: locator
                        .chat_id
                        .parse()
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
                    message_id: locator
                        .message_id
                        .parse()
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
                });
            }
            self.engine
                .prepare_selection(crate::model::PrepareSelectionRequest { message_refs: refs })
                .await
        } else {
            if intent.targets.len() != 1 {
                return Err(safe(ErrorCode::ScopeMismatch));
            }
            let id = chat_id(&intent.targets[0])?;
            if intent.action_id == "delete_my_messages" && intent.actor.is_none() {
                self.engine.prepare_own_messages(id).await
            } else if intent.action_id == "delete_by_sender" {
                let actor = intent.actor.ok_or_else(|| safe(ErrorCode::ScopeMismatch))?;
                validate_refs(
                    context,
                    std::slice::from_ref(&actor),
                    &TelegramPayloadValidator,
                    Some(ResourceKind::Actor),
                )?;
                let locator: TelegramActorLocator =
                    serde_json::from_value(actor.resource.locator_payload)
                        .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                let sender_id: i64 = locator
                    .native_id
                    .parse()
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?;
                // The reviewed legacy recipe encodes positive User and negative
                // Chat IDs. Reject a valid locator it cannot preserve rather than
                // silently changing its tagged identity during plan binding.
                if (sender_id > 0) != (locator.kind == super::locators::TelegramActorKind::User) {
                    return Err(safe(ErrorCode::UnsupportedSchema));
                }
                self.engine
                    .prepare_sender_action(crate::model::PrepareSenderActionRequest {
                        chat_id: id,
                        sender_id,
                    })
                    .await
            } else {
                if intent.actor.is_some() {
                    return Err(safe(ErrorCode::ScopeMismatch));
                }
                let operation = match intent.action_id.as_str() {
                    "clear_history" => cleaner_domain::PlanOperation::ClearHistory,
                    "remove_chat_for_self" => cleaner_domain::PlanOperation::RemoveChatForSelf,
                    "leave_chat" => cleaner_domain::PlanOperation::LeaveChat,
                    "delete_group" => cleaner_domain::PlanOperation::DeleteGroup,
                    _ => return Err(safe(ErrorCode::UnsupportedSchema)),
                };
                self.engine
                    .prepare_chat_action(crate::model::PrepareChatActionRequest {
                        chat_id: id,
                        operation,
                    })
                    .await
            }
        }
        .map_err(boundary_error)?;
        self.check_active(context)?;
        self.engine.reviewed_plan(view.id).map_err(boundary_error)
    }
    async fn authorize(
        &self,
        context: &ActiveContext,
        plan: ReviewedPlanRef,
    ) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.engine
            .authorize_plan(crate::model::AuthorizePlanRequest {
                plan_id: plan.plan_id,
                fingerprint: plan.fingerprint,
            })
            .await
            .map_err(boundary_error)?;
        self.check_active(context)
    }
    async fn start(
        &self,
        context: &ActiveContext,
        request: StartReviewed,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        self.check_active(context)?;
        let job = self
            .engine
            .start_execution(crate::model::ExecuteRequest {
                plan_id: request.plan_id,
                fingerprint: request.fingerprint,
                irreversible_acknowledged: request.irreversible_acknowledged,
                typed_chat_title: request.typed_chat_title,
            })
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        self.normalize_live_job(&job)
    }
    async fn jobs(
        &self,
        context: &ActiveContext,
    ) -> Result<Vec<retract_domain::ScopedJobRecord>, SafeError> {
        self.check_active(context)?;
        let jobs = self.engine.jobs().await;
        self.check_active(context)?;
        jobs.iter().map(|j| self.normalize_live_job(j)).collect()
    }
    async fn cancel(
        &self,
        context: &ActiveContext,
        job_id: uuid::Uuid,
    ) -> Result<retract_domain::ScopedJobRecord, SafeError> {
        self.check_active(context)?;
        let job = self
            .engine
            .cancel_job(job_id)
            .await
            .map_err(boundary_error)?;
        self.check_active(context)?;
        self.normalize_live_job(&job)
    }
    async fn recover(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check_active(context)?;
        self.engine.resume_incomplete().await;
        self.check_active(context)
    }
    async fn has_workers(&self) -> bool {
        self.engine.has_workers().await
    }
    async fn stop(&self) {
        self.engine.stop_workers().await;
    }
}

pub struct TelegramConnection {
    gateway: Arc<LiveGateway>,
    store: Arc<FoundationStore>,
}
impl TelegramConnection {
    pub fn new(gateway: Arc<LiveGateway>, store: Arc<FoundationStore>) -> Arc<Self> {
        Arc::new(Self { gateway, store })
    }
}
#[async_trait]
impl ApplicationConnection for TelegramConnection {
    fn context(&self) -> Option<ActiveContext> {
        self.gateway.active_context()
    }
    fn store(&self) -> Option<Arc<FoundationStore>> {
        Some(self.store.clone())
    }
    fn bootstrap(&self) -> Result<wire::BootstrapSnapshot, SafeError> {
        let identity = match self.gateway.identity_verification_status() {
            IdentityVerificationStatus::Unavailable => wire::IdentityStatus::Unavailable,
            IdentityVerificationStatus::Pending => wire::IdentityStatus::Pending,
            IdentityVerificationStatus::Ready => wire::IdentityStatus::Ready,
            IdentityVerificationStatus::Failed { diagnostic } => {
                wire::IdentityStatus::Failed { diagnostic }
            }
        };
        let progress = self.gateway.catalog_progress();
        let mut auth = self.gateway.auth();
        // The legacy auth error may include provider text; v2 exposes predefined copy.
        if matches!(auth.stage, crate::model::AuthStage::Error) {
            auth.hint = Some("Telegram could not continue. Check settings and retry.".into());
        }
        Ok(wire::BootstrapSnapshot {
            identity,
            auth: Some(retract_domain::VersionedPayload {
                schema: "telegram.auth".into(),
                version: 1,
                payload: serde_json::to_value(auth)
                    .map_err(|_| safe(ErrorCode::UnsupportedSchema))?,
            }),
            catalog: wire::CatalogProgress {
                phase: progress.phase.into(),
                total: progress.total,
                processed: progress.processed,
            },
            chats: vec![],
            recent_jobs: vec![],
            legacy_history: self
                .store
                .snapshot()
                .map_err(boundary_error)?
                .legacy_history
                .into_iter()
                .map(|r| r.record)
                .collect(),
        })
    }
    async fn registration(&self) -> Result<Arc<dyn ProviderRegistration>, SafeError> {
        let active = self
            .context()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        let identity = self
            .gateway
            .verified_identity()
            .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
        let context = Arc::new(
            EngineContext::new(active.clone(), identity, self.gateway.session_binding())
                .map_err(boundary_error)?,
        );
        let repository = Arc::new(
            FoundationTelegramRepository::new(self.store.clone(), active.scope)
                .map_err(boundary_error)?,
        );
        let engine = CleanerService::new_scoped(self.gateway.clone(), context.clone(), repository)
            .map_err(boundary_error)?;
        Ok(Arc::new(
            TelegramCompatibilityProvider::new(self.gateway.clone(), context, engine)
                .map_err(boundary_error)?,
        ))
    }
    async fn auth(&self, request: wire::AuthRequest) -> Result<(), SafeError> {
        let value = request.value.as_deref().unwrap_or("");
        match request.operation.as_str() {
            "request_qr_auth" if request.value.is_none() => self.gateway.request_qr_auth().await,
            "submit_phone" => self.gateway.submit_phone(value).await,
            "submit_email_address" => self.gateway.submit_email_address(value).await,
            "submit_email_code" => self.gateway.submit_email_code(value).await,
            "submit_code" => self.gateway.submit_code(value).await,
            "submit_password" => self.gateway.submit_password(value).await,
            _ => return Err(safe(ErrorCode::UnsupportedSchema)),
        }
        .map_err(boundary_error)
    }
    async fn retry_identity(&self) -> Result<(), SafeError> {
        self.gateway.retry_identity_verification()
    }
    async fn shutdown(&self) {
        let _ = self.gateway.close().await;
    }
}
