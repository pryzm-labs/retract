use super::authorization::{authorization_reason, trusted_prompt_label};
use super::*;
use crate::{
    demo_gateway::{DemoGateway, TestFailurePoint},
    providers::telegram::native::ports::{TelegramMutation, TelegramRead},
    secure_store::SecureJobStore,
};
use cleaner_domain::{ContentKind, MessageSnapshot, PlanOperation};

const TERMINAL_JOB_TIMEOUT: Duration = Duration::from_secs(10);

async fn prepared_broad_restart_plan(
    service: &Arc<TelegramCleanup>,
    operation: PlanOperation,
) -> DeletionPlan {
    let view = match operation {
        PlanOperation::ClearHistory => service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1001,
                operation,
            })
            .await
            .unwrap(),
        PlanOperation::ClearHistoryAndLeave => service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1002,
                operation: PlanOperation::LeaveChat,
            })
            .await
            .unwrap(),
        PlanOperation::RemoveChatForSelf => service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: 304,
                operation,
            })
            .await
            .unwrap(),
        PlanOperation::DeleteBySender => service
            .prepare_sender_action(PrepareSenderActionRequest {
                chat_id: -1001,
                sender_id: 714,
            })
            .await
            .unwrap(),
        PlanOperation::DeleteGroup => service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1001,
                operation,
            })
            .await
            .unwrap(),
        _ => panic!("unsupported broad restart operation: {operation:?}"),
    };
    let plan = service.plans.read().await.get(&view.id).cloned().unwrap();
    assert_eq!(plan.operation, operation);
    plan
}

async fn wait_for_terminal_job(service: &TelegramCleanup, job_id: Uuid) -> JobRecord {
    tokio::time::timeout(TERMINAL_JOB_TIMEOUT, async {
        loop {
            let job = service
                .legacy_jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job_id)
                .unwrap();
            if job.status.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "job {job_id} did not become terminal within {} seconds",
            TERMINAL_JOB_TIMEOUT.as_secs()
        )
    })
}

async fn wait_for_persisted_terminal_job(
    path: &std::path::Path,
    key: [u8; 32],
    job_id: Uuid,
) -> PersistedState {
    tokio::time::timeout(TERMINAL_JOB_TIMEOUT, async {
        loop {
            let state = SecureJobStore::with_test_key(path.to_path_buf(), key)
                .load()
                .unwrap();
            if state
                .jobs
                .iter()
                .any(|job| job.id == job_id && job.status.is_terminal())
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "job {job_id} was not durably terminal within {} seconds",
            TERMINAL_JOB_TIMEOUT.as_secs()
        )
    })
}

async fn wait_for_rate_limited_job(service: &TelegramCleanup, job_id: Uuid) -> JobRecord {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let job = service
                .legacy_jobs()
                .await
                .into_iter()
                .find(|candidate| candidate.id == job_id)
                .unwrap();
            if job.status == JobStatus::Queued && job.retry_after_seconds.is_some() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("job {job_id} did not enter its bounded retry wait"))
}

async fn wait_for_current_reach_check(gateway: &DemoGateway) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if gateway.current_reach_started() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the synthetic reach preflight did not start");
}

#[test]
fn extracts_bounded_telegram_retry_delays() {
    assert_eq!(
        telegram_retry_after(&AppError::Gateway(
            "429 Too Many Requests: retry after 17".into()
        )),
        Some(17)
    );
    assert_eq!(
        telegram_retry_after(&AppError::Gateway("FLOOD_WAIT_999999".into())),
        Some(86_400)
    );
    assert_eq!(
        telegram_retry_after(&AppError::Gateway("CHAT_ADMIN_REQUIRED".into())),
        None
    );
}

#[test]
fn persisted_cleanup_state_contains_recovery_metadata_but_no_message_content() {
    const PRIVATE_PREVIEW: &str = "SYNTHETIC_CONTENT_MUST_NOT_PERSIST";

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("jobs.enc");
    let key = [34; 32];
    let plan = DeletionPlan::selected_messages(vec![MessageSnapshot {
        chat_id: -1001,
        message_id: 90_001,
        sender_id: 714,
        sender_name: "Synthetic Sender".into(),
        sent_at: Utc::now(),
        is_outgoing: false,
        content_kind: ContentKind::File,
        preview: PRIVATE_PREVIEW.into(),
        privacy_findings: Vec::new(),
        album_id: Some(81_001),
        is_pinned: false,
        deletion_reach: DeletionReach::Everyone,
    }])
    .unwrap();
    let mut job = JobRecord::new(&plan);
    job.status = JobStatus::Partial;
    job.deleted = 1;
    job.next_batch = 1;
    job.retry_after_seconds = Some(17);
    job.error_codes = vec!["synthetic_retry_exhausted".into()];
    job.updated_at = job.created_at + chrono::Duration::seconds(5);

    SecureJobStore::with_test_key(path.clone(), key)
        .save(&PersistedState {
            plans: vec![plan.clone()],
            jobs: vec![job.clone()],
        })
        .unwrap();
    let reloaded = SecureJobStore::with_test_key(path, key).load().unwrap();

    assert_eq!(reloaded.plans.len(), 1);
    assert_eq!(reloaded.jobs.len(), 1);
    let reloaded_plan = &reloaded.plans[0];
    let reloaded_job = &reloaded.jobs[0];
    assert_eq!(reloaded_plan.id, plan.id);
    assert_eq!(reloaded_plan.fingerprint, plan.fingerprint);
    assert_eq!(reloaded_plan.operation, PlanOperation::SelectedMessages);
    assert_eq!(reloaded_plan.items.len(), 1);
    assert_eq!(reloaded_plan.items[0].chat_id, -1001);
    assert_eq!(reloaded_plan.items[0].message_id, 90_001);
    assert_eq!(
        reloaded_plan.items[0].expected_reach,
        DeletionReach::Everyone
    );
    assert_eq!(reloaded_plan.summary.selected, 1);
    assert_eq!(reloaded_plan.summary.delete_for_everyone, 1);
    assert_eq!(reloaded_plan.created_at, plan.created_at);
    assert_eq!(reloaded_job.id, job.id);
    assert_eq!(reloaded_job.plan_id, plan.id);
    assert_eq!(reloaded_job.operation, PlanOperation::SelectedMessages);
    assert_eq!(reloaded_job.target_chat_ids, vec![-1001]);
    assert_eq!(reloaded_job.status, JobStatus::Partial);
    assert_eq!(
        (
            reloaded_job.total,
            reloaded_job.deleted,
            reloaded_job.skipped,
            reloaded_job.failed,
            reloaded_job.next_batch,
        ),
        (1, 1, 0, 0, 1)
    );
    assert_eq!(reloaded_job.retry_after_seconds, Some(17));
    assert_eq!(reloaded_job.error_codes, vec!["synthetic_retry_exhausted"]);
    assert_eq!(reloaded_job.created_at, job.created_at);
    assert_eq!(reloaded_job.updated_at, job.updated_at);

    let serialized = serde_json::to_string(&reloaded).unwrap();
    assert!(!serialized.contains(PRIVATE_PREVIEW));
    for forbidden in [
        "preview",
        "text",
        "caption",
        "fileName",
        "attachment",
        "apiHash",
        "password",
        "authCode",
    ] {
        assert!(!serialized.contains(&format!("\"{forbidden}\"")));
    }
}

#[test]
fn high_impact_plan_requires_a_bound_single_use_grant() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [19; 32]);
        let gateway: Arc<DemoGateway> = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway, store).unwrap();
        let chat = service
            .read
            .chats()
            .await
            .unwrap()
            .into_iter()
            .find(|chat| chat.capabilities.can_clear_for_everyone)
            .unwrap();
        let plan = service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: chat.id,
                operation: PlanOperation::ClearHistory,
            })
            .await
            .unwrap();
        let execution = || ExecuteRequest {
            plan_id: plan.id,
            fingerprint: plan.fingerprint.clone(),
            irreversible_acknowledged: true,
            typed_chat_title: plan.chat_title.clone(),
        };

        assert!(matches!(
            service.start_execution(execution()).await,
            Err(AppError::SystemAuthentication(_))
        ));
        assert!(
            service
                .authorize_plan(AuthorizePlanRequest {
                    plan_id: plan.id,
                    fingerprint: "altered".into(),
                })
                .await
                .is_err()
        );
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        let job = service.start_execution(execution()).await.unwrap();

        wait_for_terminal_job(&service, job.id).await;
        assert!(matches!(
            service.start_execution(execution()).await,
            Err(AppError::SystemAuthentication(_))
        ));
        assert_eq!(service.legacy_jobs().await.len(), 1);
    });
}

#[test]
fn selected_message_plan_requires_a_bound_single_use_grant() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [21; 32]);
        let gateway: Arc<DemoGateway> = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway, store).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: 101,
                    message_id: 1,
                }],
            })
            .await
            .unwrap();
        assert_eq!(
            plan.confirmation_tier,
            cleaner_domain::ConfirmationTier::Low
        );
        let execution = || ExecuteRequest {
            plan_id: plan.id,
            fingerprint: plan.fingerprint.clone(),
            irreversible_acknowledged: true,
            typed_chat_title: None,
        };

        assert!(matches!(
            service.start_execution(execution()).await,
            Err(AppError::SystemAuthentication(_))
        ));
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        assert!(service.start_execution(execution()).await.is_ok());
    });
}

#[test]
fn native_authorization_reason_identifies_the_exact_frozen_target() {
    tauri::async_runtime::block_on(async {
        let gateway = DemoGateway::new();
        let chat = gateway.chat_by_id(-1001).await.unwrap().unwrap();
        let plan = DeletionPlan::by_sender(&chat, 714, "Priya".into()).unwrap();
        let reason = authorization_reason(&plan);

        assert!(reason.contains("Design Team"));
        assert!(reason.contains("-1001"));
        assert!(reason.contains("Priya"));
        assert!(reason.contains("714"));
        assert!(reason.contains(&plan.fingerprint[..12]));
    });
}

#[test]
fn native_authorization_labels_strip_line_and_direction_controls() {
    assert_eq!(
        trusted_prompt_label("Design\nTeam \u{202e}123\u{2069}"),
        "Design Team 123"
    );
}

#[test]
fn sender_plan_uses_the_backend_resolved_display_name() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [22; 32]);
        let gateway: Arc<DemoGateway> = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway, store).unwrap();
        let plan = service
            .prepare_sender_action(PrepareSenderActionRequest {
                chat_id: -1001,
                sender_id: 714,
            })
            .await
            .unwrap();

        assert_eq!(plan.target_sender_name.as_deref(), Some("Priya"));
    });
}

#[test]
fn cancellation_during_capability_refresh_prevents_the_destructive_call() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [24; 32]);
        let gateway = Arc::new(DemoGateway::new());
        gateway.delay_current_reach(100);
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: 101,
                    message_id: 1,
                }],
            })
            .await
            .unwrap();
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: None,
            })
            .await
            .unwrap();

        wait_for_current_reach_check(&gateway).await;
        service.cancel_job(job.id).await.unwrap();
        let finished = wait_for_terminal_job(&service, job.id).await;
        assert_eq!(finished.status, JobStatus::Cancelled);
        assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
    });
}

#[test]
fn selected_everyone_deletion_never_downgrades_after_reach_changes() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [32; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: 101,
                    message_id: 1,
                }],
            })
            .await
            .unwrap();

        gateway
            .set_message_reach(101, 1, DeletionReach::SelfOnly)
            .await;
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        gateway.clear_operation_log().await;
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: None,
            })
            .await
            .unwrap();

        wait_for_terminal_job(&service, job.id).await;

        let finished = service
            .legacy_jobs()
            .await
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap();
        let operations = gateway.operation_log().await;
        assert!(
            operations.is_empty(),
            "unexpected destructive operations: {operations:?}"
        );
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 0);
        assert_eq!(finished.skipped, 1);
        assert_eq!(finished.failed, 0);
        assert!(finished.error_codes.is_empty());
        assert!(finished.retry_after_seconds.is_none());
        assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
    });
}

#[test]
fn selected_everyone_deletion_rechecks_reach_after_a_rate_limit_wait() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [41; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: 101,
                    message_id: 1,
                }],
            })
            .await
            .unwrap();
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        gateway.clear_test_traces().await;
        gateway
            .inject_rate_limit_once(TestFailurePoint::DeleteMessagesForEveryone)
            .await;

        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: None,
            })
            .await
            .unwrap();
        gateway.wait_for_injected_failure(1).await;
        let waiting = wait_for_rate_limited_job(&service, job.id).await;
        assert_eq!(waiting.retry_after_seconds, Some(1));
        assert_eq!(waiting.error_codes, ["telegram_rate_limited"]);

        gateway
            .set_message_reach(101, 1, DeletionReach::SelfOnly)
            .await;
        let finished = wait_for_terminal_job(&service, job.id).await;

        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 0);
        assert_eq!(finished.skipped, 1);
        assert_eq!(finished.failed, 0);
        assert_eq!(finished.next_batch, 1);
        assert!(finished.retry_after_seconds.is_none());
        assert_eq!(finished.error_codes, ["telegram_rate_limited"]);
        assert_eq!(
            gateway.current_reach_calls().await,
            vec![(101, 1), (101, 1)]
        );
        assert_eq!(
            gateway.operation_log().await,
            vec!["delete_messages_for_everyone:101:1"]
        );
        assert_eq!(gateway.delete_calls().await, vec![(101, vec![1])]);
        assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
    });
}

#[test]
fn chat_wide_deletion_rechecks_authority_after_a_rate_limit_wait() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [42; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1001,
                operation: PlanOperation::ClearHistory,
            })
            .await
            .unwrap();
        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        gateway.clear_test_traces().await;
        gateway
            .inject_rate_limit_once(TestFailurePoint::ClearHistoryForEveryone)
            .await;

        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: plan.chat_title,
            })
            .await
            .unwrap();
        gateway.wait_for_injected_failure(1).await;
        let waiting = wait_for_rate_limited_job(&service, job.id).await;
        assert_eq!(waiting.status, JobStatus::Queued);
        assert_eq!(waiting.retry_after_seconds, Some(1));
        assert_eq!(waiting.error_codes, ["telegram_rate_limited"]);

        gateway.set_chat_clear_authority(-1001, false).await;
        let finished = wait_for_terminal_job(&service, job.id).await;

        assert_eq!(finished.status, JobStatus::Failed);
        assert_eq!(finished.total, 0);
        assert_eq!(finished.deleted, 0);
        assert_eq!(finished.skipped, 0);
        assert_eq!(finished.failed, 0);
        assert_eq!(finished.next_batch, 0);
        assert!(finished.retry_after_seconds.is_none());
        assert_eq!(
            finished.error_codes,
            ["telegram_rate_limited", "telegram_rejected"]
        );
        assert_eq!(gateway.chat_by_id_calls().await, vec![-1001, -1001]);
        assert_eq!(
            gateway.operation_log().await,
            vec!["clear_history_for_everyone:-1001"]
        );
        assert!(gateway.chat_by_id(-1001).await.unwrap().is_some());
    });
}

#[test]
fn selected_message_job_uses_exact_telegram_batch_boundaries() {
    tauri::async_runtime::block_on(async {
        const CHAT_ID: i64 = 101;
        const FIRST_MESSAGE_ID: i64 = 50_000;
        const MESSAGE_COUNT: usize = 205;

        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [33; 32]);
        let gateway = Arc::new(DemoGateway::new());
        gateway
            .append_messages(CHAT_ID, FIRST_MESSAGE_ID, MESSAGE_COUNT)
            .await;
        let expected_refs = (0..MESSAGE_COUNT)
            .map(|offset| MessageRef {
                chat_id: CHAT_ID,
                message_id: FIRST_MESSAGE_ID + offset as i64,
            })
            .collect::<Vec<_>>();
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_selection(PrepareSelectionRequest {
                message_refs: expected_refs.clone(),
            })
            .await
            .unwrap();

        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        gateway.clear_operation_log().await;
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: None,
            })
            .await
            .unwrap();

        wait_for_terminal_job(&service, job.id).await;

        let finished = service
            .legacy_jobs()
            .await
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap();
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, MESSAGE_COUNT);
        assert_eq!(finished.skipped, 0);
        assert_eq!(finished.failed, 0);
        assert!(finished.error_codes.is_empty());
        assert!(finished.retry_after_seconds.is_none());
        assert_eq!(gateway.delete_batch_sizes().await, vec![100, 100, 5]);
        assert_eq!(
            gateway.delete_calls().await,
            vec![
                (CHAT_ID, (50_000..=50_099).collect::<Vec<_>>()),
                (CHAT_ID, (50_100..=50_199).collect::<Vec<_>>()),
                (CHAT_ID, (50_200..=50_204).collect::<Vec<_>>()),
            ]
        );

        let expected_operations = expected_refs
            .chunks(100)
            .map(|batch| {
                let message_ids = batch
                    .iter()
                    .map(|message| message.message_id.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("delete_messages_for_everyone:{CHAT_ID}:{message_ids}")
            })
            .collect::<Vec<_>>();
        assert_eq!(gateway.operation_log().await, expected_operations);
    });
}

#[test]
fn restart_requires_new_review_for_every_non_idempotent_broad_operation() {
    tauri::async_runtime::block_on(async {
        for operation in [
            PlanOperation::ClearHistory,
            PlanOperation::ClearHistoryAndLeave,
            PlanOperation::RemoveChatForSelf,
            PlanOperation::DeleteBySender,
            PlanOperation::DeleteGroup,
        ] {
            for (persisted_status, deleted, expected_status) in [
                (JobStatus::Queued, 0, JobStatus::Failed),
                (JobStatus::Running, 1, JobStatus::Partial),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("jobs.enc");
                let key = [35; 32];
                let gateway = Arc::new(DemoGateway::new());
                let preparation = TelegramCleanup::new(
                    gateway.clone(),
                    SecureJobStore::with_test_key(path.clone(), key),
                )
                .unwrap();
                let plan = prepared_broad_restart_plan(&preparation, operation).await;
                let mut job = JobRecord::new(&plan);
                job.status = persisted_status;
                job.total = 1;
                job.deleted = deleted;
                let job_id = job.id;
                drop(preparation);
                SecureJobStore::with_test_key(path.clone(), key)
                    .save(&PersistedState {
                        plans: vec![plan],
                        jobs: vec![job],
                    })
                    .unwrap();

                gateway.clear_operation_log().await;
                let service = TelegramCleanup::new(
                    gateway.clone(),
                    SecureJobStore::with_test_key(path.clone(), key),
                )
                .unwrap();
                service.resume_incomplete().await;

                let interrupted = service
                    .legacy_jobs()
                    .await
                    .into_iter()
                    .find(|candidate| candidate.id == job_id)
                    .unwrap();
                assert_eq!(
                    interrupted.status, expected_status,
                    "unexpected restart status for {operation:?} from {persisted_status:?}"
                );
                assert_eq!(interrupted.deleted, deleted);
                assert_eq!(interrupted.retry_after_seconds, None);
                assert_eq!(
                    interrupted.error_codes,
                    vec!["restart_requires_new_review"],
                    "unexpected restart diagnostic for {operation:?} from {persisted_status:?}"
                );
                assert!(
                    gateway.operation_log().await.is_empty(),
                    "restart replayed {operation:?} from {persisted_status:?}"
                );

                let reloaded = SecureJobStore::with_test_key(path, key).load().unwrap();
                assert_eq!(reloaded.jobs.len(), 1);
                assert_eq!(reloaded.jobs[0].status, expected_status);
                assert_eq!(
                    reloaded.jobs[0].error_codes,
                    vec!["restart_requires_new_review"]
                );
            }
        }
    });
}

#[test]
fn restart_resumes_selected_message_job_from_frozen_ids() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [36; 32];
        let gateway = Arc::new(DemoGateway::new());
        let preparation = TelegramCleanup::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), key),
        )
        .unwrap();
        let view = preparation
            .prepare_selection(PrepareSelectionRequest {
                message_refs: vec![MessageRef {
                    chat_id: 101,
                    message_id: 1,
                }],
            })
            .await
            .unwrap();
        let plan = preparation
            .plans
            .read()
            .await
            .get(&view.id)
            .cloned()
            .unwrap();
        let mut job = JobRecord::new(&plan);
        job.status = JobStatus::Running;
        let job_id = job.id;
        drop(preparation);
        SecureJobStore::with_test_key(path.clone(), key)
            .save(&PersistedState {
                plans: vec![plan],
                jobs: vec![job],
            })
            .unwrap();

        gateway.clear_operation_log().await;
        let service =
            TelegramCleanup::new(gateway.clone(), SecureJobStore::with_test_key(path, key))
                .unwrap();
        service.resume_incomplete().await;

        let finished = wait_for_terminal_job(&service, job_id).await;
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 1);
        assert!(
            finished
                .error_codes
                .iter()
                .any(|code| code == "resumed_after_restart")
        );
        assert_eq!(
            gateway.operation_log().await,
            vec!["delete_messages_for_everyone:101:1"]
        );
        assert!(
            gateway
                .messages_by_ids(&[(101, 1)])
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn restart_resumes_selected_message_job_from_nonzero_batch_cursor() {
    tauri::async_runtime::block_on(async {
        const CHAT_ID: i64 = 101;
        const FIRST_MESSAGE_ID: i64 = 70_000;
        const MESSAGE_COUNT: usize = 205;
        const COMPLETED_BATCH_SIZE: usize = 100;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [38; 32];
        let gateway = Arc::new(DemoGateway::new());
        gateway
            .append_messages(CHAT_ID, FIRST_MESSAGE_ID, MESSAGE_COUNT)
            .await;
        let message_refs = (0..MESSAGE_COUNT)
            .map(|offset| MessageRef {
                chat_id: CHAT_ID,
                message_id: FIRST_MESSAGE_ID + offset as i64,
            })
            .collect::<Vec<_>>();
        let preparation = TelegramCleanup::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), key),
        )
        .unwrap();
        let view = preparation
            .prepare_selection(PrepareSelectionRequest {
                message_refs: message_refs.clone(),
            })
            .await
            .unwrap();
        let plan = preparation
            .plans
            .read()
            .await
            .get(&view.id)
            .cloned()
            .unwrap();

        let completed_ids = message_refs[..COMPLETED_BATCH_SIZE]
            .iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>();
        gateway
            .delete_messages_for_everyone(CHAT_ID, &completed_ids)
            .await
            .unwrap();

        let mut job = JobRecord::new(&plan);
        job.status = JobStatus::Running;
        job.deleted = COMPLETED_BATCH_SIZE;
        job.next_batch = 1;
        let job_id = job.id;
        drop(preparation);
        SecureJobStore::with_test_key(path.clone(), key)
            .save(&PersistedState {
                plans: vec![plan.clone()],
                jobs: vec![job],
            })
            .unwrap();

        gateway.clear_test_traces().await;
        let service = TelegramCleanup::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), key),
        )
        .unwrap();
        service.resume_incomplete().await;

        let finished = wait_for_terminal_job(&service, job_id).await;
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.total, MESSAGE_COUNT);
        assert_eq!(finished.deleted, MESSAGE_COUNT);
        assert_eq!(finished.skipped, 0);
        assert_eq!(finished.failed, 0);
        assert_eq!(finished.next_batch, 3);
        assert_eq!(finished.retry_after_seconds, None);
        assert_eq!(finished.error_codes, vec!["resumed_after_restart"]);
        assert_eq!(gateway.delete_batch_sizes().await, vec![100, 5]);
        assert_eq!(
            gateway.delete_calls().await,
            vec![
                (CHAT_ID, (70_100..=70_199).collect::<Vec<_>>()),
                (CHAT_ID, (70_200..=70_204).collect::<Vec<_>>()),
            ]
        );
        assert!(
            gateway
                .delete_calls()
                .await
                .iter()
                .flat_map(|(_, message_ids)| message_ids)
                .all(|message_id| *message_id >= 70_100)
        );

        let reloaded = wait_for_persisted_terminal_job(&path, key, job_id).await;
        assert_eq!(reloaded.plans, vec![plan]);
        assert_eq!(reloaded.jobs.len(), 1);
        assert_eq!(reloaded.jobs[0].status, JobStatus::Completed);
        assert_eq!(reloaded.jobs[0].deleted, MESSAGE_COUNT);
        assert_eq!(reloaded.jobs[0].next_batch, 3);
        assert_eq!(reloaded.jobs[0].error_codes, vec!["resumed_after_restart"]);
    });
}

#[test]
fn restart_resumes_own_message_job_from_frozen_ids() {
    tauri::async_runtime::block_on(async {
        const NEW_OWN_MESSAGE_ID: i64 = 60_001;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [37; 32];
        let gateway = Arc::new(DemoGateway::new());
        let preparation = TelegramCleanup::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), key),
        )
        .unwrap();
        let view = preparation.prepare_own_messages(-1003).await.unwrap();
        let plan = preparation
            .plans
            .read()
            .await
            .get(&view.id)
            .cloned()
            .unwrap();
        assert_eq!(plan.operation, PlanOperation::DeleteMyMessages);
        gateway.append_messages(-1003, NEW_OWN_MESSAGE_ID, 1).await;
        let mut job = JobRecord::new(&plan);
        job.status = JobStatus::Running;
        let job_id = job.id;
        drop(preparation);
        SecureJobStore::with_test_key(path.clone(), key)
            .save(&PersistedState {
                plans: vec![plan],
                jobs: vec![job],
            })
            .unwrap();

        gateway.clear_operation_log().await;
        let service =
            TelegramCleanup::new(gateway.clone(), SecureJobStore::with_test_key(path, key))
                .unwrap();
        service.resume_incomplete().await;

        let finished = wait_for_terminal_job(&service, job_id).await;
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 1);
        assert!(
            finished
                .error_codes
                .iter()
                .any(|code| code == "resumed_after_restart")
        );
        assert_eq!(
            gateway.operation_log().await,
            vec!["delete_messages_for_everyone:-1003:31"]
        );
        assert!(
            gateway
                .messages_by_ids(&[(-1003, 31)])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            gateway
                .messages_by_ids(&[(-1003, NEW_OWN_MESSAGE_ID)])
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn chat_scoped_plans_never_load_the_global_catalog() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [23; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();

        service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1001,
                operation: PlanOperation::ClearHistory,
            })
            .await
            .unwrap();
        service
            .prepare_sender_action(PrepareSenderActionRequest {
                chat_id: -1001,
                sender_id: 714,
            })
            .await
            .unwrap();
        let own_plan = service.prepare_own_messages(-1003).await.unwrap();
        assert_eq!(own_plan.operation, PlanOperation::DeleteMyMessages);
        assert_eq!(own_plan.summary.delete_for_everyone, 1);

        assert_eq!(gateway.chat_read_counts(), (0, 3));
    });
}

#[test]
fn admin_leave_job_deletes_every_eligible_message_before_removing_membership() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [29; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();

        let plan = service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1003,
                operation: PlanOperation::LeaveChat,
            })
            .await
            .unwrap();
        assert_eq!(plan.operation, PlanOperation::DeleteAllMessagesAndLeave);
        assert_eq!(plan.summary.selected, 3);
        assert_eq!(plan.summary.delete_for_everyone, 1);
        assert_eq!(plan.summary.cannot_delete, 2);
        assert_eq!(
            plan.confirmation_tier,
            cleaner_domain::ConfirmationTier::High
        );

        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        gateway.clear_operation_log().await;
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: plan.chat_title,
            })
            .await
            .unwrap();

        wait_for_terminal_job(&service, job.id).await;

        let finished = service
            .legacy_jobs()
            .await
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap();
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 1);
        assert_eq!(finished.skipped, 2);
        assert_eq!(finished.failed, 0);
        assert!(finished.error_codes.is_empty());
        assert!(finished.retry_after_seconds.is_none());
        assert!(gateway.chat_by_id(-1003).await.unwrap().is_none());
        assert!(
            gateway
                .messages_by_ids(&[(-1003, 31)])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            gateway
                .messages_by_ids(&[(-1003, 32), (-1003, 33)])
                .await
                .unwrap()
                .len(),
            2
        );
        let operations = gateway.operation_log().await;
        assert_eq!(
            operations,
            vec![
                "delete_messages_for_everyone:-1003:31",
                "leave_chat:-1003",
                "remove_chat_for_self:-1003",
            ]
        );
    });
}

#[test]
fn leave_job_finishes_local_removal_when_membership_is_already_gone() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [30; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();
        let plan = service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1003,
                operation: PlanOperation::LeaveChat,
            })
            .await
            .unwrap();

        gateway.leave_chat(-1003).await.unwrap();
        assert!(gateway.chat_by_id(-1003).await.unwrap().is_some());

        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: plan.chat_title,
            })
            .await
            .unwrap();

        wait_for_terminal_job(&service, job.id).await;

        let finished = service
            .legacy_jobs()
            .await
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap();
        assert_eq!(finished.status, JobStatus::Completed);
        assert!(gateway.chat_by_id(-1003).await.unwrap().is_none());
    });
}

#[test]
fn leave_plan_favors_whole_history_cleanup_when_telegram_allows_it() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store = SecureJobStore::with_test_key(directory.path().join("jobs.enc"), [31; 32]);
        let gateway = Arc::new(DemoGateway::new());
        let service = TelegramCleanup::new(gateway.clone(), store).unwrap();

        let plan = service
            .prepare_chat_action(PrepareChatActionRequest {
                chat_id: -1002,
                operation: PlanOperation::LeaveChat,
            })
            .await
            .unwrap();

        assert_eq!(format!("{:?}", plan.operation), "ClearHistoryAndLeave");
        assert_eq!(
            plan.confirmation_tier,
            cleaner_domain::ConfirmationTier::High
        );

        service
            .authorize_plan(AuthorizePlanRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint.clone(),
            })
            .await
            .unwrap();
        let job = service
            .start_execution(ExecuteRequest {
                plan_id: plan.id,
                fingerprint: plan.fingerprint,
                irreversible_acknowledged: true,
                typed_chat_title: plan.chat_title,
            })
            .await
            .unwrap();
        wait_for_terminal_job(&service, job.id).await;

        let finished = service
            .legacy_jobs()
            .await
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap();
        assert_eq!(finished.status, JobStatus::Completed);
        assert!(gateway.chat_by_id(-1002).await.unwrap().is_none());
        assert!(
            gateway
                .messages_by_ids(&[(-1002, 21), (-1002, 22), (-1002, 23), (-1002, 24)])
                .await
                .unwrap()
                .is_empty()
        );
    });
}
