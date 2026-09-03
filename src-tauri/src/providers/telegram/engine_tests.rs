use std::sync::Arc;

use cleaner_domain::{DeletionPlan, DeletionReach, PlanOperation};
use retract_domain::{ExpectedEffect, RestartPolicy};
use uuid::Uuid;

use crate::{
    demo_gateway::DemoGateway,
    gateway::TelegramGateway,
    model::{AuthorizePlanRequest, ExecuteRequest, MessageRef, PrepareSelectionRequest},
    persistence::{FoundationStore, ProviderPayloadValidator, StoreBinding},
    service::CleanerService,
};

use super::{
    compat::{TelegramCompatibilityProvider, TelegramExecutionRecipe},
    engine_context::{EngineContext, FoundationTelegramRepository, TelegramStateRepository},
    identity::{SessionBinding, TelegramAccountProfile, VerifiedTelegramIdentity},
    locators::{TelegramEnvironment, TelegramPayloadValidator, telegram_provider_key},
};

struct ObservedGateway {
    inner: Arc<DemoGateway>,
    binding: Arc<SessionBinding>,
    invalidate_after_delete: bool,
    mutation_error: Option<&'static str>,
    preflight_error: Option<&'static str>,
    left: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait::async_trait]
impl TelegramGateway for ObservedGateway {
    fn info(&self) -> crate::gateway::GatewayInfo {
        self.inner.info()
    }
    fn auth(&self) -> crate::model::AuthSnapshot {
        self.inner.auth()
    }
    fn verified_identity(&self) -> Option<VerifiedTelegramIdentity> {
        self.inner.verified_identity()
    }
    fn catalog_progress(&self) -> crate::model::CatalogProgress {
        self.inner.catalog_progress()
    }
    async fn chats(&self) -> Result<Vec<cleaner_domain::ChatSummary>, crate::error::AppError> {
        self.inner.chats().await
    }
    async fn chat_by_id(
        &self,
        chat_id: i64,
    ) -> Result<Option<cleaner_domain::ChatSummary>, crate::error::AppError> {
        self.inner.chat_by_id(chat_id).await
    }
    async fn search(
        &self,
        request: &crate::model::SearchRequest,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, crate::error::AppError> {
        self.inner.search(request).await
    }
    async fn own_messages(
        &self,
        chat_id: i64,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, crate::error::AppError> {
        self.inner.own_messages(chat_id).await
    }
    async fn chat_messages(
        &self,
        chat_id: i64,
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, crate::error::AppError> {
        self.inner.chat_messages(chat_id).await
    }
    async fn messages_by_ids(
        &self,
        ids: &[(i64, i64)],
    ) -> Result<Vec<cleaner_domain::MessageSnapshot>, crate::error::AppError> {
        self.inner.messages_by_ids(ids).await
    }
    async fn sender_name(&self, sender_id: i64) -> Result<String, crate::error::AppError> {
        self.inner.sender_name(sender_id).await
    }
    async fn current_reach(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<DeletionReach>, crate::error::AppError> {
        if let Some(error) = self.preflight_error {
            return Err(crate::error::AppError::Gateway(error.into()));
        }
        self.inner.current_reach(chat_id, message_id).await
    }
    async fn clear_history_for_everyone(&self, chat_id: i64) -> Result<(), crate::error::AppError> {
        self.inner.clear_history_for_everyone(chat_id).await
    }
    async fn clear_history_for_everyone_keep_chat(
        &self,
        chat_id: i64,
    ) -> Result<(), crate::error::AppError> {
        self.inner
            .clear_history_for_everyone_keep_chat(chat_id)
            .await?;
        if let Some(error) = self.mutation_error {
            return Err(crate::error::AppError::Gateway(error.into()));
        }
        Ok(())
    }
    async fn remove_chat_for_self(&self, chat_id: i64) -> Result<(), crate::error::AppError> {
        self.inner.remove_chat_for_self(chat_id).await
    }
    async fn delete_group(&self, chat_id: i64) -> Result<(), crate::error::AppError> {
        self.inner.delete_group(chat_id).await
    }
    async fn delete_messages_by_sender(
        &self,
        chat_id: i64,
        sender_id: i64,
    ) -> Result<(), crate::error::AppError> {
        self.inner
            .delete_messages_by_sender(chat_id, sender_id)
            .await
    }
    async fn request_qr_auth(&self) -> Result<(), crate::error::AppError> {
        self.inner.request_qr_auth().await
    }
    async fn submit_phone(&self, phone: &str) -> Result<(), crate::error::AppError> {
        self.inner.submit_phone(phone).await
    }
    async fn submit_email_address(&self, email: &str) -> Result<(), crate::error::AppError> {
        self.inner.submit_email_address(email).await
    }
    async fn submit_email_code(&self, code: &str) -> Result<(), crate::error::AppError> {
        self.inner.submit_email_code(code).await
    }
    async fn submit_code(&self, code: &str) -> Result<(), crate::error::AppError> {
        self.inner.submit_code(code).await
    }
    async fn submit_password(&self, password: &str) -> Result<(), crate::error::AppError> {
        self.inner.submit_password(password).await
    }
    async fn close(&self) -> Result<(), crate::error::AppError> {
        self.inner.close().await
    }
    async fn delete_messages_for_everyone(
        &self,
        chat_id: i64,
        message_ids: &[i64],
    ) -> Result<(), crate::error::AppError> {
        if let Some(error) = self.mutation_error
            && !matches!(
                error,
                "TDLIB_REQUEST_TIMEOUT" | "TDLIB_RESPONSE_CHANNEL_CLOSED"
            )
        {
            return Err(crate::error::AppError::Gateway(error.into()));
        }
        self.inner
            .delete_messages_for_everyone(chat_id, message_ids)
            .await?;
        if self.invalidate_after_delete {
            self.binding.invalidate();
        }
        if let Some(error) = self.mutation_error {
            return Err(crate::error::AppError::Gateway(error.into()));
        }
        Ok(())
    }
    async fn leave_chat(&self, chat_id: i64) -> Result<(), crate::error::AppError> {
        self.inner.leave_chat(chat_id).await?;
        self.left.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: Arc<FoundationStore>,
    binding: Arc<SessionBinding>,
    context: Arc<EngineContext>,
    gateway: Arc<DemoGateway>,
    repository: Arc<FoundationTelegramRepository>,
    service: Arc<CleanerService>,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let store = FoundationStore::open_with_test_key_and_payload_validator(
        directory.path().join("profile"),
        StoreBinding {
            provider: telegram_provider_key(),
            profile: "engine-tests".into(),
        },
        [0x65; 32],
        Arc::new(TelegramPayloadValidator),
    )
    .unwrap();
    let identity =
        VerifiedTelegramIdentity::new(TelegramEnvironment::Test, 42, Uuid::new_v4()).unwrap();
    let binding = Arc::new(SessionBinding::default());
    binding
        .begin_generation(identity.session_generation)
        .unwrap();
    let active = binding
        .publish(
            &store,
            &identity,
            &TelegramAccountProfile {
                display_name: "Synthetic account".into(),
                username: None,
            },
        )
        .unwrap();
    let context = Arc::new(EngineContext::new(active, identity.clone(), binding.clone()).unwrap());
    let gateway = Arc::new(DemoGateway::with_verified_identity(identity));
    let repository = Arc::new(
        FoundationTelegramRepository::new(store.clone(), context.active().scope.clone()).unwrap(),
    );
    let service =
        CleanerService::new_scoped(gateway.clone(), context.clone(), repository.clone()).unwrap();
    Fixture {
        _directory: directory,
        store,
        binding,
        context,
        gateway,
        repository,
        service,
    }
}

async fn selection(f: &Fixture) -> crate::model::PlanView {
    f.service
        .prepare_selection(PrepareSelectionRequest {
            message_refs: vec![MessageRef {
                chat_id: -1001,
                message_id: 14,
            }],
        })
        .await
        .unwrap()
}

#[test]
fn scoped_constructor_rejects_repository_for_a_different_verified_account() {
    let first = fixture();
    let second = fixture();
    assert!(
        CleanerService::new_scoped(
            first.gateway.clone(),
            first.context.clone(),
            second.repository.clone()
        )
        .is_err()
    );
}

#[test]
fn compatibility_provider_cannot_wrap_an_engine_from_another_scope() {
    let first = fixture();
    let second = fixture();
    assert!(
        TelegramCompatibilityProvider::new(
            first.gateway.clone(),
            first.context.clone(),
            second.service.clone()
        )
        .is_err()
    );
}

#[test]
fn validated_store_rejects_skipping_unprocessed_batches_and_foreign_dirty_targets() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let view = selection(&f).await;
        let legacy = f.repository.load().unwrap().plans.remove(0);
        let mut job = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &crate::model::JobRecord::new(&legacy),
            true,
        )
        .unwrap();
        job.next_batch = 1;
        assert!(
            f.store
                .transaction(|state| {
                    state.jobs.push(job.clone());
                    Ok(())
                })
                .is_err()
        );
        job.next_batch = 0;
        job.dirty_refs = vec![super::compat::conversation_ref(&job.scope, -999).unwrap()];
        assert!(
            f.store
                .transaction(|state| {
                    state.jobs.push(job);
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(f.store.snapshot().unwrap().plans[0].id, view.id);
    });
}

#[test]
fn normalization_preserves_telegram_filter_metadata_and_permission_truth() {
    use crate::providers::ports::{ActionRequest, ContentQuery, QuerySource};
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let provider = TelegramCompatibilityProvider::new(
            f.gateway.clone(),
            f.context.clone(),
            f.service.clone(),
        )
        .unwrap();
        let result = provider
            .search(ContentQuery {
                scope: f.context.active().scope.clone(),
                query: String::new(),
                cursor: None,
                limit: 100,
            })
            .await
            .unwrap();
        let photo = result
            .items
            .iter()
            .find(|item| {
                item.resource.locator_payload["messageId"] == "13"
                    && item.resource.locator_payload["chatId"] == "-1001"
            })
            .unwrap();
        assert_eq!(photo.kind, retract_domain::ContentKind::Image);
        let metadata: super::compat::TelegramContentMetadata =
            serde_json::from_value(photo.provider_metadata.as_ref().unwrap().payload.clone())
                .unwrap();
        assert_eq!(metadata.original_kind, cleaner_domain::ContentKind::Photo);
        assert!(!metadata.outgoing);
        assert!(metadata.grouping.is_some());
        let target = retract_domain::ScopedResourceRef {
            scope: photo.scope.clone(),
            id: *photo.id.as_uuid(),
            resource: photo.resource.clone(),
        };
        let actions = provider
            .actions_for(ActionRequest {
                context: f.context.active().clone(),
                targets: vec![target],
            })
            .await
            .unwrap();
        assert_eq!(
            actions[0].descriptors[0].effect,
            ExpectedEffect::RemovedForAllParticipants
        );
        let mut foreign = f.context.active().scope.clone();
        foreign.source_id = Uuid::new_v4().try_into().unwrap();
        assert!(
            provider
                .search(ContentQuery {
                    scope: foreign,
                    query: String::new(),
                    cursor: None,
                    limit: 100
                })
                .await
                .is_err()
        );
    });
}

#[test]
fn scoped_plan_installs_one_fingerprint_before_review_and_persistence() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let view = selection(&f).await;
        let snapshot = f.store.snapshot().unwrap();
        assert_eq!(snapshot.plans.len(), 1);
        let plan = &snapshot.plans[0];
        assert_eq!(view.fingerprint, plan.fingerprint);
        assert_eq!(plan.scope, f.context.active().scope);
        let legacy = f.repository.load().unwrap().plans.remove(0);
        assert_eq!(legacy.fingerprint, plan.fingerprint);
        assert_eq!(plan.restart_policy, RestartPolicy::ResumeFrozenTargets);
        let recipe = TelegramExecutionRecipe::from_envelope(plan).unwrap();
        assert!(
            !serde_json::to_string(&recipe)
                .unwrap()
                .contains("fingerprint")
        );
        let mut foreign = legacy;
        let mut other_scope = plan.scope.clone();
        other_scope.source_id = Uuid::new_v4().try_into().unwrap();
        let other = TelegramCompatibilityProvider::bind_plan(&other_scope, &mut foreign).unwrap();
        assert_ne!(other.fingerprint, plan.fingerprint);
        let mut changed = plan.clone();
        changed.steps[0].descriptor.effect = ExpectedEffect::RemovedForCurrentAccountOnly;
        changed.seal().unwrap();
        assert!(TelegramPayloadValidator.validate_recipe(&changed).is_err());
    });
}

#[test]
fn stale_binding_rejects_grant_and_execution_without_gateway_mutation() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let plan = selection(&f).await;
        f.service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        f.binding.invalidate();
        assert!(
            f.service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None,
                })
                .await
                .is_err()
        );
        assert!(f.gateway.delete_calls().await.is_empty());
        assert!(f.service.jobs().await.is_empty());
    });
}

#[test]
fn failed_plan_save_does_not_publish_review_or_executable_state() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        std::fs::create_dir(f._directory.path().join("profile/jobs.enc.tmp")).unwrap();
        assert!(
            f.service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: -1001,
                        message_id: 14
                    }],
                })
                .await
                .is_err()
        );
        assert!(f.repository.load().unwrap().plans.is_empty());
        assert!(f.store.snapshot().unwrap().plans.is_empty());
        assert!(f.gateway.delete_calls().await.is_empty());
    });
}

#[test]
fn compound_cleanup_maps_each_ordered_effect_and_critical_destruction_separately() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let chat = f.gateway.chat_by_id(-1002).await.unwrap().unwrap();
        let mut legacy = DeletionPlan::leave_chat(&chat, vec![]).unwrap();
        let plan = TelegramCompatibilityProvider::bind_plan(&f.context.active().scope, &mut legacy)
            .unwrap();
        assert_eq!(
            plan.steps
                .iter()
                .map(|step| step.descriptor.effect)
                .collect::<Vec<_>>(),
            vec![
                ExpectedEffect::RemovedForAllParticipants,
                ExpectedEffect::MembershipRemoved,
                ExpectedEffect::RemovedForCurrentAccountOnly,
            ]
        );
        assert_eq!(plan.restart_policy, RestartPolicy::RequiresNewReview);
        let chat = f.gateway.chat_by_id(-1001).await.unwrap().unwrap();
        let mut destroy = DeletionPlan::chat_wide(PlanOperation::DeleteGroup, &chat).unwrap();
        let plan =
            TelegramCompatibilityProvider::bind_plan(&f.context.active().scope, &mut destroy)
                .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.steps[0].descriptor.effect,
            ExpectedEffect::ContainerDestroyed
        );
        assert_eq!(
            plan.confirmation.tier,
            retract_domain::ConfirmationTier::Critical
        );
    });
}

#[test]
fn normalized_counts_preserve_unavailable_selections_outside_eligible_total() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let mut messages = f.gateway.messages_by_ids(&[(-1001, 14)]).await.unwrap();
        let mut protected = messages[0].clone();
        protected.message_id = 900;
        protected.deletion_reach = DeletionReach::SelfOnly;
        messages.push(protected);
        let mut legacy = DeletionPlan::selected_messages(messages).unwrap();
        TelegramCompatibilityProvider::bind_plan(&f.context.active().scope, &mut legacy).unwrap();
        let mut job = crate::model::JobRecord::new(&legacy);
        job.deleted = 1;
        job.status = crate::model::JobStatus::Completed;
        let normalized = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &job,
            true,
        )
        .unwrap();
        assert_eq!(normalized.counters.selected, 2);
        assert_eq!(normalized.counters.eligible, 1);
        assert_eq!(normalized.counters.deleted, 1);
        assert_eq!(normalized.counters.skipped, 1);
    });
}

async fn start(
    service: &Arc<CleanerService>,
    plan: &crate::model::PlanView,
) -> crate::model::JobRecord {
    service
        .authorize_plan(AuthorizePlanRequest {
            plan_id: plan.id,
            fingerprint: plan.fingerprint.clone(),
        })
        .await
        .unwrap();
    service
        .start_execution(ExecuteRequest {
            plan_id: plan.id,
            fingerprint: plan.fingerprint.clone(),
            irreversible_acknowledged: true,
            typed_chat_title: plan.chat_title.clone(),
        })
        .await
        .unwrap()
}

async fn terminal(service: &CleanerService, id: Uuid) -> crate::model::JobRecord {
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if let Some(job) = service
                .jobs()
                .await
                .into_iter()
                .find(|j| j.id == id && j.status.is_terminal())
            {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

struct Prompt {
    opened: tokio::sync::Notify,
    release: tokio::sync::Notify,
    count: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::engine_context::TestOwnerPrompt for Prompt {
    async fn authenticate(&self) -> Result<(), crate::error::AppError> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.opened.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[test]
fn identity_change_during_owner_prompt_cannot_publish_a_grant() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let prompt = Arc::new(Prompt {
            opened: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            count: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut context = EngineContext::new(
            f.context.active().clone(),
            f.gateway.verified_identity().unwrap(),
            f.binding.clone(),
        )
        .unwrap();
        context.owner_prompt = Some(prompt.clone());
        let service =
            CleanerService::new_scoped(f.gateway.clone(), Arc::new(context), f.repository.clone())
                .unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: -1001,
                    message_id: 14,
                }],
            })
            .await
            .unwrap();
        let task = {
            let service = service.clone();
            let fingerprint = plan.fingerprint.clone();
            tokio::spawn(async move {
                service
                    .authorize_plan(AuthorizePlanRequest {
                        plan_id: plan.id,
                        fingerprint,
                    })
                    .await
            })
        };
        prompt.opened.notified().await;
        f.binding.invalidate();
        prompt.release.notify_one();
        assert!(task.await.unwrap().is_err());
        assert_eq!(prompt.count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            service
                .start_execution(ExecuteRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint,
                    irreversible_acknowledged: true,
                    typed_chat_title: None
                })
                .await
                .is_err()
        );
        assert!(f.gateway.delete_calls().await.is_empty());
        assert!(service.jobs().await.is_empty());
    });
}

struct FailProgress {
    inner: Arc<FoundationTelegramRepository>,
}
impl TelegramStateRepository for FailProgress {
    fn scope(&self) -> Option<&retract_domain::Scope> {
        self.inner.scope()
    }
    fn load(&self) -> Result<crate::model::PersistedState, crate::error::AppError> {
        self.inner.load()
    }
    fn save(&self, state: &crate::model::PersistedState) -> Result<(), crate::error::AppError> {
        if state.jobs.iter().any(|job| job.next_batch > 0) {
            return Err(crate::error::AppError::StatePersistenceFailed);
        }
        self.inner.save(state)
    }
}

struct FailAfterLeave {
    inner: Arc<FoundationTelegramRepository>,
    left: Arc<std::sync::atomic::AtomicBool>,
}
impl TelegramStateRepository for FailAfterLeave {
    fn scope(&self) -> Option<&retract_domain::Scope> {
        self.inner.scope()
    }
    fn load(&self) -> Result<crate::model::PersistedState, crate::error::AppError> {
        self.inner.load()
    }
    fn save(&self, state: &crate::model::PersistedState) -> Result<(), crate::error::AppError> {
        if self.left.load(std::sync::atomic::Ordering::Acquire) {
            return Err(crate::error::AppError::StatePersistenceFailed);
        }
        self.inner.save(state)
    }
}

#[test]
fn failed_membership_progress_save_prevents_following_local_removal() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let left = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = Arc::new(ObservedGateway {
            inner: f.gateway.clone(),
            binding: f.binding.clone(),
            invalidate_after_delete: false,
            mutation_error: None,
            preflight_error: None,
            left: left.clone(),
        });
        let service = CleanerService::new_scoped(
            observed,
            f.context.clone(),
            Arc::new(FailAfterLeave {
                inner: f.repository.clone(),
                left,
            }),
        )
        .unwrap();
        let plan = service
            .prepare_chat_action(crate::model::PrepareChatActionRequest {
                chat_id: -1002,
                operation: PlanOperation::LeaveChat,
            })
            .await
            .unwrap();
        let job = start(&service, &plan).await;
        let failed = terminal(&service, job.id).await;
        assert!(
            failed
                .error_codes
                .iter()
                .any(|c| c == "state_persistence_failed")
        );
        let operations = f.gateway.operation_log().await;
        assert!(operations.iter().any(|op| op.starts_with("leave_chat:")));
        assert!(
            !operations
                .iter()
                .any(|op| op.starts_with("remove_chat_for_self:")),
            "{operations:?}"
        );
    });
}

#[test]
fn identity_change_during_a_mutation_records_uncertainty_and_stops() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let observed = Arc::new(ObservedGateway {
            inner: f.gateway.clone(),
            binding: f.binding.clone(),
            invalidate_after_delete: true,
            mutation_error: None,
            preflight_error: None,
            left: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let service =
            CleanerService::new_scoped(observed, f.context.clone(), f.repository.clone()).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: -1001,
                    message_id: 14,
                }],
            })
            .await
            .unwrap();
        let job = start(&service, &plan).await;
        let failed = terminal(&service, job.id).await;
        assert_eq!(failed.deleted, 0);
        assert_eq!(failed.uncertain, 1);
        assert_eq!(failed.failed, 0);
        let persisted = &f.store.snapshot().unwrap().jobs[0];
        assert_eq!(persisted.counters.uncertain, 1);
        assert!(persisted.status.is_terminal());
        assert_eq!(
            persisted.diagnostics[0].code,
            retract_domain::ErrorCode::AmbiguousOutcome
        );
    });
}

#[test]
fn failed_progress_save_stops_before_next_batch_and_does_not_publish_cursor() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        f.gateway.append_messages(-1001, 1000, 101).await;
        let service = CleanerService::new_scoped(
            f.gateway.clone(),
            f.context.clone(),
            Arc::new(FailProgress {
                inner: f.repository.clone(),
            }),
        )
        .unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: (1000..1101)
                    .map(|id| MessageRef {
                        chat_id: -1001,
                        message_id: id,
                    })
                    .collect(),
            })
            .await
            .unwrap();
        let job = start(&service, &plan).await;
        let failed = terminal(&service, job.id).await;
        assert_eq!(failed.error_codes, vec!["state_persistence_failed"]);
        assert_eq!(failed.next_batch, 0);
        assert_eq!(f.gateway.delete_batch_sizes().await, vec![100]);
        assert_eq!(f.store.snapshot().unwrap().jobs[0].next_batch, 0);
        assert!(
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: plan.fingerprint
                })
                .await
                .is_err()
        );
    });
}

#[test]
fn rate_wait_rechecks_binding_before_any_retry_call() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let plan = selection(&f).await;
        f.gateway
            .inject_rate_limit_once(
                crate::demo_gateway::TestFailurePoint::DeleteMessagesForEveryone,
            )
            .await;
        let job = start(&f.service, &plan).await;
        f.gateway.wait_for_injected_failure(1).await;
        f.binding.invalidate();
        let failed = terminal(&f.service, job.id).await;
        assert_eq!(failed.status, crate::model::JobStatus::Failed);
        assert_eq!(f.gateway.delete_calls().await.len(), 1);
        assert_eq!(f.gateway.current_reach_calls().await.len(), 1);
    });
}

#[test]
fn recovery_only_resumes_matching_authorized_frozen_jobs_and_blocks_foreign_scope() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let plan = selection(&f).await;
        let legacy = f.repository.load().unwrap().plans.remove(0);
        let queued = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &crate::model::JobRecord::new(&legacy),
            true,
        )
        .unwrap();
        f.store
            .transaction(|s| {
                s.jobs.push(queued.clone());
                Ok(())
            })
            .unwrap();
        let next_repository = Arc::new(
            FoundationTelegramRepository::new(f.store.clone(), f.context.active().scope.clone())
                .unwrap(),
        );
        let resumed =
            CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), next_repository)
                .unwrap();
        resumed.resume_incomplete().await;
        assert_eq!(terminal(&resumed, queued.id).await.deleted, 1);
        assert_eq!(f.gateway.delete_calls().await, vec![(-1001, vec![14])]);
        assert_eq!(
            plan.fingerprint,
            f.store.snapshot().unwrap().plans[0].fingerprint
        );

        let f = fixture();
        selection(&f).await;
        let legacy = f.repository.load().unwrap().plans.remove(0);
        let queued = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &crate::model::JobRecord::new(&legacy),
            true,
        )
        .unwrap();
        f.store
            .transaction(|s| {
                s.jobs.push(queued);
                Ok(())
            })
            .unwrap();
        let identity =
            VerifiedTelegramIdentity::new(TelegramEnvironment::Test, 43, Uuid::new_v4()).unwrap();
        f.binding
            .begin_generation(identity.session_generation)
            .unwrap();
        let active = f
            .binding
            .publish(
                &f.store,
                &identity,
                &TelegramAccountProfile {
                    display_name: "Other".into(),
                    username: None,
                },
            )
            .unwrap();
        let gateway = Arc::new(DemoGateway::with_verified_identity(identity.clone()));
        let repository = Arc::new(
            FoundationTelegramRepository::new(f.store.clone(), active.scope.clone()).unwrap(),
        );
        let service = CleanerService::new_scoped(
            gateway.clone(),
            Arc::new(EngineContext::new(active, identity, f.binding.clone()).unwrap()),
            repository,
        )
        .unwrap();
        service.resume_incomplete().await;
        assert!(service.jobs().await.is_empty());
        assert!(gateway.delete_calls().await.is_empty());
        assert_eq!(
            f.store.snapshot().unwrap().jobs[0].status,
            retract_domain::JobStatus::Blocked
        );
    });
}

#[test]
fn stale_scope_projection_cannot_overwrite_newer_plan_or_cursor() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let stale =
            FoundationTelegramRepository::new(f.store.clone(), f.context.active().scope.clone())
                .unwrap();
        let snapshot = stale.load().unwrap();
        let plan = selection(&f).await;
        assert!(stale.save(&snapshot).is_err());
        assert_eq!(f.store.snapshot().unwrap().plans[0].id, plan.id);
    });
}

#[test]
fn two_services_sharing_repository_cannot_drop_each_others_plans() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        let stale =
            CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), f.repository.clone())
                .unwrap();
        let first = selection(&f).await;
        assert!(
            stale
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: -1001,
                        message_id: 15
                    }]
                })
                .await
                .is_err()
        );
        assert_eq!(f.store.snapshot().unwrap().plans[0].id, first.id);
    });
}

#[test]
fn uncertain_results_do_not_reclassify_earlier_confirmed_failures() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        f.gateway.append_messages(-1001, 2000, 3).await;
        let mut legacy = DeletionPlan::selected_messages(
            f.gateway
                .messages_by_ids(&[(-1001, 2000), (-1001, 2001), (-1001, 2002)])
                .await
                .unwrap(),
        )
        .unwrap();
        TelegramCompatibilityProvider::bind_plan(&f.context.active().scope, &mut legacy).unwrap();
        let mut job = crate::model::JobRecord::new(&legacy);
        job.status = crate::model::JobStatus::Partial;
        job.failed = 2;
        job.uncertain = 1;
        job.error_codes.push("ambiguous_outcome".into());
        let normalized = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &job,
            true,
        )
        .unwrap();
        assert_eq!(normalized.counters.failed, 2);
        assert_eq!(normalized.counters.uncertain, 1);
    });
}

// A post-send transport failure must not be counted as a confirmed rejection
// or allow a second batch/compound membership mutation. The synthetic gateway
// deliberately applies the first native mutation, then loses its response.
#[test]
fn post_send_transport_failures_stop_batches_and_compound_cleanup_as_uncertain() {
    tauri::async_runtime::block_on(async {
        for error in ["TDLIB_REQUEST_TIMEOUT", "TDLIB_RESPONSE_CHANNEL_CLOSED"] {
            for operation in [
                PlanOperation::SelectedMessages,
                PlanOperation::DeleteAllMessagesAndLeave,
                PlanOperation::ClearHistoryAndLeave,
            ] {
                let f = fixture();
                let chat_id = match operation {
                    PlanOperation::SelectedMessages => -1001,
                    PlanOperation::DeleteAllMessagesAndLeave => -1003,
                    _ => -1002,
                };
                f.gateway.append_messages(chat_id, 2000, 101).await;
                let left = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let observed = Arc::new(ObservedGateway {
                    inner: f.gateway.clone(),
                    binding: f.binding.clone(),
                    invalidate_after_delete: false,
                    mutation_error: Some(error),
                    preflight_error: None,
                    left: left.clone(),
                });
                let service =
                    CleanerService::new_scoped(observed, f.context.clone(), f.repository.clone())
                        .unwrap();
                let plan = if operation == PlanOperation::SelectedMessages {
                    service
                        .prepare_selection(PrepareSelectionRequest {
                            message_refs: (2000..2101)
                                .map(|id| MessageRef {
                                    chat_id: -1001,
                                    message_id: id,
                                })
                                .collect(),
                        })
                        .await
                        .unwrap()
                } else {
                    service
                        .prepare_chat_action(crate::model::PrepareChatActionRequest {
                            chat_id,
                            operation: PlanOperation::LeaveChat,
                        })
                        .await
                        .unwrap()
                };
                assert_eq!(plan.operation, operation);
                let job = start(&service, &plan).await;
                let stopped = terminal(&service, job.id).await;
                assert!(
                    stopped.error_codes.iter().any(|c| c == "ambiguous_outcome"),
                    "{error}: {operation:?}: {stopped:?}"
                );
                assert_eq!(stopped.failed, 0);
                assert_eq!(stopped.deleted, 0);
                if operation != PlanOperation::ClearHistoryAndLeave {
                    assert_eq!(stopped.uncertain, 100);
                    assert_eq!(f.gateway.delete_batch_sizes().await, vec![100]);
                }
                assert!(!left.load(std::sync::atomic::Ordering::Acquire));
                let persisted = f.store.snapshot().unwrap().jobs.remove(0);
                assert!(
                    persisted
                        .diagnostics
                        .iter()
                        .any(|d| d.code == retract_domain::ErrorCode::AmbiguousOutcome)
                );
                assert!(persisted.retry_at.is_none());
                let repository = Arc::new(
                    FoundationTelegramRepository::new(
                        f.store.clone(),
                        f.context.active().scope.clone(),
                    )
                    .unwrap(),
                );
                let recovered =
                    CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), repository)
                        .unwrap();
                let calls = f.gateway.delete_calls().await;
                recovered.resume_incomplete().await;
                assert_eq!(f.gateway.delete_calls().await, calls);
            }
        }
    });
}

#[test]
fn preflight_transport_timeout_and_confirmed_rejection_are_not_ambiguous() {
    tauri::async_runtime::block_on(async {
        for (preflight, error) in [
            (true, "TDLIB_REQUEST_TIMEOUT"),
            (true, "TDLIB_RESPONSE_CHANNEL_CLOSED"),
            (false, "400 MESSAGE_DELETE_FORBIDDEN"),
        ] {
            let f = fixture();
            let observed = Arc::new(ObservedGateway {
                inner: f.gateway.clone(),
                binding: f.binding.clone(),
                invalidate_after_delete: false,
                mutation_error: (!preflight).then_some(error),
                preflight_error: preflight.then_some(error),
                left: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            let service =
                CleanerService::new_scoped(observed, f.context.clone(), f.repository.clone())
                    .unwrap();
            let plan = service
                .prepare_selection(PrepareSelectionRequest {
                    message_refs: vec![MessageRef {
                        chat_id: -1001,
                        message_id: 14,
                    }],
                })
                .await
                .unwrap();
            let job = start(&service, &plan).await;
            let stopped = terminal(&service, job.id).await;
            assert_eq!(stopped.uncertain, 0);
            assert_eq!(stopped.failed, 1);
            assert!(!stopped.error_codes.iter().any(|c| c == "ambiguous_outcome"));
            if preflight {
                assert!(f.gateway.delete_calls().await.is_empty());
                assert_eq!(stopped.error_codes, vec!["telegram_timeout"]);
            } else {
                assert_eq!(stopped.error_codes, vec!["telegram_rejected"]);
            }
        }
    });
}

#[test]
fn recovery_preserves_absolute_retry_deadline_and_guards_the_remaining_wait() {
    tauri::async_runtime::block_on(async {
        for outcome in ["resume", "cancel", "switch"] {
            let f = fixture();
            selection(&f).await;
            let legacy = f.repository.load().unwrap().plans.remove(0);
            let mut queued = TelegramCompatibilityProvider::normalize_job(
                &f.context.active().scope,
                &legacy,
                &crate::model::JobRecord::new(&legacy),
                true,
            )
            .unwrap();
            let deadline = chrono::Utc::now() + chrono::Duration::seconds(2);
            queued.retry_at = Some(deadline);
            queued.diagnostics.push(retract_domain::SafeError {
                code: retract_domain::ErrorCode::RateLimited,
                retry_at: Some(deadline),
            });
            f.store
                .transaction(|s| {
                    s.jobs.push(queued.clone());
                    Ok(())
                })
                .unwrap();
            let repository = Arc::new(
                FoundationTelegramRepository::new(
                    f.store.clone(),
                    f.context.active().scope.clone(),
                )
                .unwrap(),
            );
            let service =
                CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), repository)
                    .unwrap();
            assert_eq!(
                f.store.snapshot().unwrap().jobs[0].retry_at,
                Some(deadline),
                "constructor must not erase/rebase the deadline"
            );
            service.resume_incomplete().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(chrono::Utc::now() < deadline);
            assert!(f.gateway.current_reach_calls().await.is_empty());
            assert!(f.gateway.delete_calls().await.is_empty());
            assert_eq!(f.store.snapshot().unwrap().jobs[0].retry_at, Some(deadline));
            match outcome {
                "cancel" => {
                    service.cancel_job(queued.id).await.unwrap();
                }
                "switch" => f.binding.invalidate(),
                _ => {}
            }
            let stopped = terminal(&service, queued.id).await;
            if outcome == "resume" {
                assert!(chrono::Utc::now() >= deadline);
                assert_eq!(stopped.deleted, 1);
                assert_eq!(f.gateway.delete_calls().await, vec![(-1001, vec![14])]);
            } else {
                assert!(f.gateway.delete_calls().await.is_empty());
                assert!(f.gateway.current_reach_calls().await.is_empty());
                assert_eq!(
                    stopped.status,
                    if outcome == "cancel" {
                        crate::model::JobStatus::Cancelled
                    } else {
                        crate::model::JobStatus::Failed
                    }
                );
            }
            assert!(f.store.snapshot().unwrap().jobs[0].retry_at.is_none());
        }
    });
}

#[test]
fn terminal_safe_diagnostics_round_trip_without_provider_text_or_code_loss() {
    use retract_domain::ErrorCode::*;
    tauri::async_runtime::block_on(async {
        let f = fixture();
        selection(&f).await;
        let legacy = f.repository.load().unwrap().plans.remove(0);
        let mut job = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &crate::model::JobRecord::new(&legacy),
            true,
        )
        .unwrap();
        job.status = retract_domain::JobStatus::Failed;
        let deadline = chrono::Utc::now();
        job.diagnostics = [
            AuthenticationRequired,
            PermissionChanged,
            NotFound,
            AlreadyRemoved,
            RateLimited,
            CostLimitReached,
            Transient,
            Permanent,
            AmbiguousOutcome,
            UnsupportedSchema,
            InvalidArchive,
            UnsupportedContractVersion,
            ScopeMismatch,
            StaleContext,
            IdentityUnavailable,
            ProfileInUse,
            StatePersistenceFailed,
            MigrationRequiresNewReview,
            RestartRequiresNewReview,
        ]
        .into_iter()
        .map(|code| retract_domain::SafeError {
            code,
            retry_at: (code == RateLimited).then_some(deadline),
        })
        .collect();
        f.store
            .transaction(|s| {
                s.jobs.push(job.clone());
                Ok(())
            })
            .unwrap();
        let repository = Arc::new(
            FoundationTelegramRepository::new(f.store.clone(), f.context.active().scope.clone())
                .unwrap(),
        );
        CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), repository).unwrap();
        assert_eq!(
            f.store.snapshot().unwrap().jobs[0].diagnostics,
            job.diagnostics
        );
        let mut legacy_job = crate::model::JobRecord::new(&legacy);
        legacy_job.error_codes = vec![
            "telegram_rate_limited".into(),
            "telegram_timeout".into(),
            "not_found".into(),
            "private provider text".into(),
        ];
        let normalized = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &legacy_job,
            true,
        )
        .unwrap();
        assert_eq!(
            normalized
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            vec![RateLimited, Transient, NotFound, PermissionChanged]
        );
        assert!(
            !serde_json::to_string(&normalized)
                .unwrap()
                .contains("private provider text")
        );
    });
}

#[test]
fn broad_or_unauthorized_recovery_requires_review_and_descriptive_recipes_never_execute() {
    tauri::async_runtime::block_on(async {
        for broad in [false, true] {
            let f = fixture();
            if broad {
                f.service
                    .prepare_chat_action(crate::model::PrepareChatActionRequest {
                        chat_id: -1001,
                        operation: PlanOperation::ClearHistory,
                    })
                    .await
                    .unwrap();
            } else {
                selection(&f).await;
            }
            let legacy = f.repository.load().unwrap().plans.remove(0);
            let job = TelegramCompatibilityProvider::normalize_job(
                &f.context.active().scope,
                &legacy,
                &crate::model::JobRecord::new(&legacy),
                broad,
            )
            .unwrap();
            f.store
                .transaction(|state| {
                    state.jobs.push(job.clone());
                    Ok(())
                })
                .unwrap();
            let repository = Arc::new(
                FoundationTelegramRepository::new(
                    f.store.clone(),
                    f.context.active().scope.clone(),
                )
                .unwrap(),
            );
            let service =
                CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), repository)
                    .unwrap();
            service.resume_incomplete().await;
            let stopped = &service.jobs().await[0];
            assert!(stopped.status.is_terminal());
            assert!(
                stopped
                    .error_codes
                    .iter()
                    .any(|c| c == "restart_requires_new_review")
            );
            assert!(f.gateway.operation_log().await.is_empty());
        }
        let f = fixture();
        selection(&f).await;
        f.store
            .transaction(|state| {
                let plan = &mut state.plans[0];
                plan.recipe = super::locators::TelegramRemediationRecipe::for_steps(&plan.steps)?;
                plan.seal().unwrap();
                Ok(())
            })
            .unwrap();
        let repository = Arc::new(
            FoundationTelegramRepository::new(f.store.clone(), f.context.active().scope.clone())
                .unwrap(),
        );
        assert!(
            CleanerService::new_scoped(f.gateway.clone(), f.context.clone(), repository).is_err()
        );
        assert!(f.gateway.delete_calls().await.is_empty());
    });
}

#[test]
fn switching_account_does_not_label_potentially_inflight_work_safely_blocked() {
    tauri::async_runtime::block_on(async {
        let f = fixture();
        selection(&f).await;
        let legacy = f.repository.load().unwrap().plans.remove(0);
        let mut job = TelegramCompatibilityProvider::normalize_job(
            &f.context.active().scope,
            &legacy,
            &crate::model::JobRecord::new(&legacy),
            true,
        )
        .unwrap();
        job.status = retract_domain::JobStatus::Running;
        f.store
            .transaction(|s| {
                s.jobs.push(job);
                Ok(())
            })
            .unwrap();
        let identity =
            VerifiedTelegramIdentity::new(TelegramEnvironment::Test, 43, Uuid::new_v4()).unwrap();
        f.binding
            .begin_generation(identity.session_generation)
            .unwrap();
        let active = f
            .binding
            .publish(
                &f.store,
                &identity,
                &TelegramAccountProfile {
                    display_name: "Other".into(),
                    username: None,
                },
            )
            .unwrap();
        FoundationTelegramRepository::new(f.store.clone(), active.scope).unwrap();
        let job = &f.store.snapshot().unwrap().jobs[0];
        assert!(job.status.is_terminal());
        assert!(
            job.diagnostics
                .iter()
                .any(|d| d.code == retract_domain::ErrorCode::AmbiguousOutcome)
        );
        assert!(f.gateway.delete_calls().await.is_empty());
    });
}
