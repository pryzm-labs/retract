//! Execute the production registration through Tauri's synthetic runtime.
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

use crate::{compatibility::commands_v2, provider_service::ProviderService};

#[test]
fn compatibility_v2_content_supplies_the_exact_scoped_actor_for_sender_review() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../src/test/fixtures/telegram-ipc-contract.json"
    ))
    .unwrap();
    let message: cleaner_domain::MessageSnapshot =
        serde_json::from_value(fixture["searchResponse"]["messages"][0].clone()).unwrap();
    let scope = super::fixtures::context("context").scope;
    let record = crate::providers::telegram::compat::normalize_content(&scope, &message).unwrap();
    let metadata = record.provider_metadata.unwrap().payload;
    let actor = &metadata["actor"];
    assert_eq!(actor["scope"], serde_json::to_value(&scope).unwrap());
    assert_eq!(actor["id"], serde_json::to_value(record.author_id).unwrap());
    assert_eq!(actor["resource"]["resourceKind"], "actor");
    assert_eq!(
        actor["resource"]["locatorPayload"],
        json!({"kind":"user","nativeId":"42"})
    );
    let reference: retract_domain::ScopedResourceRef =
        serde_json::from_value(actor.clone()).unwrap();
    reference.validate(&scope).unwrap();
}

fn setup() -> tauri::WebviewWindow<MockRuntime> {
    with_service(ProviderService::setup())
}

fn with_service(service: Arc<ProviderService>) -> tauri::WebviewWindow<MockRuntime> {
    let app = commands_v2::register(mock_builder())
        .manage(Arc::new(crate::RuntimeState::new(service)))
        .build(mock_context(noop_assets()))
        .unwrap();
    tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap()
}

#[test]
fn compatibility_v2_failed_setup_exposes_safe_retryable_status_without_an_identity() {
    let webview = with_service(ProviderService::failed(crate::error::boundary_error(
        crate::error::AppError::Gateway("private bootstrap path /private/secret".into()),
    )));
    let bootstrap = invoke(
        &webview,
        "get_bootstrap_snapshot_v2",
        json!({"contractVersion":2,"context":null,"payload":{}}),
    )
    .unwrap();
    assert_eq!(bootstrap["context"], Value::Null);
    assert_eq!(bootstrap["payload"]["identity"]["state"], "failed");
    assert_eq!(
        bootstrap["payload"]["identity"]["diagnostic"]["code"],
        "permission_changed"
    );
    assert!(!bootstrap.to_string().contains("secret"));
    let error=invoke(&webview,"save_connection_settings_v2",json!({"contractVersion":2,"context":null,"payload":{"tdlibPath":"","apiId":0,"apiHash":"synthetic-invalid","useTestDc":false}})).unwrap_err();
    assert_eq!(error["code"], "scope_mismatch");
}

pub(crate) fn invoke(
    webview: &tauri::WebviewWindow<MockRuntime>,
    command: &str,
    request: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({"request": request})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .map(|body| body.deserialize().unwrap())
}

#[test]
fn compatibility_v2_setup_has_no_invented_identity_and_rejects_old_versions() {
    let webview = setup();
    let bootstrap = invoke(
        &webview,
        "get_bootstrap_snapshot_v2",
        json!({
            "contractVersion": 2, "context": null, "payload": {}
        }),
    )
    .unwrap();
    assert_eq!(bootstrap["contractVersion"], 2);
    assert_eq!(bootstrap["context"], Value::Null);
    assert_eq!(bootstrap["payload"]["identity"]["state"], "unavailable");
    for version in [0, 1, 3, 999] {
        let error = invoke(
            &webview,
            "get_bootstrap_snapshot_v2",
            json!({
                "contractVersion": version, "context": null, "payload": {}
            }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "unsupported_contract_version");
    }
}

#[test]
fn compatibility_v2_every_active_handler_requires_captured_context() {
    let webview = setup();
    for command in [
        "get_snapshot_v2",
        "search_messages_v2",
        "refresh_chats_v2",
        "prepare_selection_v2",
        "prepare_intent_v2",
        "authorize_plan_v2",
        "start_execution_v2",
        "get_jobs_v2",
        "cancel_job_v2",
    ] {
        let error = invoke(
            &webview,
            command,
            json!({
                "contractVersion": 2, "context": null, "payload": {}
            }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "identity_unavailable", "{command}: {error}");
    }
}

#[test]
fn compatibility_v2_registration_has_no_legacy_identity_or_destructive_fallback() {
    let webview = setup();
    for command in [
        "get_snapshot",
        "get_bootstrap_snapshot",
        "search_messages",
        "refresh_chats",
        "prepare_selection",
        "prepare_own_messages",
        "prepare_chat_action",
        "prepare_sender_action",
        "authorize_plan",
        "start_execution",
        "get_jobs",
        "cancel_job",
    ] {
        let error = invoke(&webview, command, json!({"chatId": -1001})).unwrap_err();
        assert_eq!(error, Value::String(format!("Command {command} not found")));
    }
}

#[test]
fn compatibility_v2_rejects_version_scope_generation_and_malformed_refs_before_effects() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let reference = fixture()["messages"][0]["ref"].clone();
        let original =
            json!({"contractVersion":2,"context":active,"payload":{"messageRefs":[reference]}});
        let cases = [
            ("/contractVersion", json!(1), "unsupported_contract_version"),
            ("/context", Value::Null, "identity_unavailable"),
            (
                "/context/scope/provider",
                json!("synthetic"),
                "scope_mismatch",
            ),
            (
                "/context/scope/accountId",
                json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
                "scope_mismatch",
            ),
            (
                "/context/scope/sourceId",
                json!("22222222-2222-4222-8222-222222222222"),
                "scope_mismatch",
            ),
            (
                "/context/sessionGeneration",
                json!("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
                "stale_context",
            ),
            (
                "/context/sessionGeneration",
                json!("not-a-uuid"),
                "scope_mismatch",
            ),
            (
                "/payload/messageRefs/0/id",
                json!("00000000-0000-0000-0000-000000000000"),
                "scope_mismatch",
            ),
            (
                "/payload/messageRefs/0/resource/locatorVersion",
                json!(99),
                "scope_mismatch",
            ),
        ];
        for (path, value, code) in cases {
            let mut raw = original.clone();
            *raw.pointer_mut(path).unwrap() = value;
            let error = invoke(&harness.webview, "prepare_selection_v2", raw).unwrap_err();
            assert_eq!(error["code"], code, "{path}: {error}");
            assert!(gateway.operation_log().await.is_empty());
            assert!(harness.store.snapshot().unwrap().plans.is_empty());
        }
        let plan = harness.prepare(&active, vec![reference]);
        harness.authorize(&active, &plan);
        let mut stale = active.clone();
        stale.session_generation = uuid::Uuid::new_v4();
        for (command, payload) in [
            ("get_jobs_v2", json!({})),
            ("cancel_job_v2", json!({"jobId":uuid::Uuid::new_v4()})),
            (
                "authorize_plan_v2",
                json!({"planId":plan.id,"fingerprint":plan.fingerprint}),
            ),
            (
                "start_execution_v2",
                json!({"planId":plan.id,"fingerprint":plan.fingerprint,"irreversibleAcknowledged":true,"typedChatTitle":null}),
            ),
        ] {
            assert_eq!(
                harness.call(command, &stale, payload).unwrap_err()["code"],
                "stale_context"
            );
        }
        assert!(gateway.operation_log().await.is_empty());
        assert!(harness.store.snapshot().unwrap().jobs.is_empty());
    });
}

#[test]
fn compatibility_v2_owned_group_cleanup_and_all_filtered_search_fields_remain_available() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let conversation = fixture()["dirtyRefs"][0].clone();
        let before = gateway.chat_read_counts();
        let intents = harness
            .call(
                "get_intents_v2",
                &active,
                json!({"actionId":"catalog","targets":[conversation],"actor":null}),
            )
            .unwrap();
        assert!(
            intents
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["actionId"] == "delete_my_messages")
        );
        let own_intent = intents
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["actionId"] == "delete_my_messages")
            .unwrap();
        assert_eq!(own_intent["descriptors"][0]["kind"], "delete_remote_item");
        assert_eq!(own_intent["descriptors"][0]["confirmationTier"], "high");
        let clear_intent = intents
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["actionId"] == "clear_history")
            .unwrap();
        assert_eq!(clear_intent["descriptors"].as_array().unwrap().len(), 1);
        assert_eq!(clear_intent["descriptors"][0]["kind"], "clear_conversation");
        let own = harness
            .call(
                "prepare_intent_v2",
                &active,
                json!({"actionId":"delete_my_messages","targets":[conversation],"actor":null}),
            )
            .unwrap();
        assert_eq!(own["recipe"]["payload"]["operation"], "delete_my_messages");
        let filters = json!({"schema":"telegram.search_filters","version":1,"payload":{"chatKinds":["supergroup"],"contentKinds":["photo"],"direction":"others","minDate":"2020-01-01T00:00:00Z","maxDate":"2030-01-01T00:00:00Z","excludePinned":true,"privacyScan":false}});
        let page = harness
            .call(
                "search_messages_v2",
                &active,
                json!({"query":"","conversations":[conversation],"filters":filters,"limit":500}),
            )
            .unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        for record in page["items"].as_array().unwrap() {
            assert_eq!(record["kind"], "image");
            assert_eq!(record["resource"]["locatorPayload"]["messageId"], "13");
            assert_eq!(record["providerMetadata"]["payload"]["outgoing"], false);
            assert_eq!(record["providerMetadata"]["payload"]["pinned"], false);
        }
        assert_eq!(gateway.chat_read_counts().0, before.0);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_blocked_foreign_history_does_not_lock_settings() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (first, _) = telegram(directory.path(), active.clone()).await;
        let plan = first.prepare(&active, vec![fixture()["messages"][0]["ref"].clone()]);
        let native =
            crate::providers::telegram::compat::TelegramExecutionRecipe::validate_envelope(&plan)
                .unwrap();
        let job = crate::providers::telegram::compat::TelegramCompatibilityProvider::normalize_job(
            &active.scope,
            &native,
            &crate::model::JobRecord::new(&native),
            true,
        )
        .unwrap();
        first
            .store
            .transaction(|s| {
                s.jobs.push(job);
                Ok(())
            })
            .unwrap();
        drop(first);
        let other = context("otherContext");
        let (next, gateway) = telegram(directory.path(), other.clone()).await;
        assert_eq!(
            next.store.snapshot().unwrap().jobs[0].status,
            retract_domain::JobStatus::Blocked
        );
        assert!(!next.service.has_workers().await);
        // Valid shape, intentionally invalid settings: must reach settings
        // validation rather than reject because foreign historical jobs exist.
        let error=invoke(&next.webview,"save_connection_settings_v2",json!({"contractVersion":2,"context":other,"payload":{"tdlibPath":"","apiId":0,"apiHash":"synthetic-invalid","useTestDc":false}})).unwrap_err();
        assert_eq!(error["code"], "scope_mismatch");
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_errors_never_copy_private_native_content() {
    use crate::error::{AppError, boundary_error};
    for error in [
        AppError::Gateway("private attachment secret.jpg".into()),
        AppError::SecureStore("/private/auth-token".into()),
        AppError::SystemAuthentication("password:123456".into()),
    ] {
        let wire = serde_json::to_value(boundary_error(error)).unwrap();
        assert_eq!(wire.as_object().unwrap().len(), 3);
        let text = wire.to_string();
        for secret in ["secret.jpg", "auth-token", "123456"] {
            assert!(!text.contains(secret));
        }
        serde_json::from_value::<retract_domain::SafeError>(wire).unwrap();
    }
}

#[test]
fn historical_v1_request_and_error_types_are_compatibility_only() {
    let request: crate::model::AuthValueRequest =
        serde_json::from_value(json!({"value":"synthetic-code"})).unwrap();
    assert_eq!(request.value, "synthetic-code");
    let error = crate::error::CommandError::from(crate::error::AppError::NotFound);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"code":"not_found","message":"the requested record was not found"})
    );
}

#[test]
fn compatibility_v2_bootstrap_discovers_verified_identity_but_mutations_require_context() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let response = invoke(
            &harness.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":null,"payload":{}}),
        )
        .unwrap();
        assert_eq!(response["context"], serde_json::to_value(&active).unwrap());
        assert_eq!(response["payload"]["identity"]["state"], "ready");
        let mut stale = active.clone();
        stale.session_generation = uuid::Uuid::new_v4();
        assert_eq!(
            harness
                .call("get_bootstrap_snapshot_v2", &stale, json!({}))
                .unwrap_err()["code"],
            "stale_context"
        );
        for command in ["submit_auth_v2", "retry_identity_v2"] {
            assert_eq!(
                invoke(
                    &harness.webview,
                    command,
                    json!({"contractVersion":2,"context":null,"payload":{}})
                )
                .unwrap_err()["code"],
                "stale_context"
            );
        }
        assert_eq!(gateway.chat_read_counts().0, 0);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_async_refresh_discards_old_context_results() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        io.invalidate_refresh
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            harness
                .call(
                    "refresh_chats_v2",
                    &active,
                    json!({"conversations":[fixture()["messages"][2]["conversation"]]})
                )
                .unwrap_err()["code"],
            "stale_context"
        );
        assert_eq!(io.refreshes.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(io.catalog.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(io.calls.lock().unwrap().is_empty());
        assert!(harness.store.snapshot().unwrap().jobs.is_empty());
    });
}

#[test]
fn compatibility_v2_nonnumeric_recovery_resumes_an_actual_durable_retry_checkpoint() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        io.rate_limit_once.store(true, Ordering::Release);
        let original = synthetic(directory.path(), io.clone());
        let plan = original.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
        original.authorize(&active, &plan);
        let job = original.start(&active, &plan).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if original.store.snapshot().unwrap().jobs.iter().any(|j| {
                    j.id == job.id
                        && j.status == retract_domain::JobStatus::Queued
                        && j.retry_at.is_some()
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(io.calls.lock().unwrap().is_empty());
        // Capture real production-written bytes at the crash point. Teardown
        // stops the old worker, then replay restores these exact bytes: no test
        // creates/seals a plan, grant, job, progress cursor or retry timestamp.
        let checkpoint = std::fs::read(directory.path().join("jobs.enc")).unwrap();
        original.service.shutdown().await;
        let previous = Arc::downgrade(&original.store);
        drop(original);
        assert!(previous.upgrade().is_none());
        std::fs::write(directory.path().join("jobs.enc"), checkpoint).unwrap();
        *io.active.write().unwrap() = Some(active.clone());
        let recovered = synthetic(directory.path(), io.clone());
        invoke(
            &recovered.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":active,"payload":{}}),
        )
        .unwrap();
        let finished = recovered.settled(&active, job.id).await;
        assert_eq!(finished.status, retract_domain::JobStatus::Completed);
        assert_eq!(finished.counters.deleted, 1);
        assert_eq!(finished.next_batch, 1);
        assert_eq!(io.calls.lock().unwrap().len(), 1);
        assert_eq!(
            io.calls.lock().unwrap()[0][0].resource.locator_payload["messageId"],
            "message:part/0007"
        );
        assert_eq!(io.catalog.load(Ordering::Acquire), 0);
        let state = encrypted_state(directory.path(), &active, KEY).unwrap();
        assert_eq!(state.jobs[0], finished);
    });
}

#[test]
fn review_round1_sender_actor_tag_cannot_change_the_reviewed_identity() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::providers::telegram::locators::{TelegramActorKind, TelegramActorLocator};
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let chat = TelegramActorLocator::new(TelegramActorKind::Chat, "714")
            .unwrap()
            .resource(active.scope.account_id);
        let user = TelegramActorLocator::new(TelegramActorKind::User, "714")
            .unwrap()
            .resource(active.scope.account_id);
        let actor = |resource: retract_domain::ProviderResourceRef| json!({"id":resource.resource_id().unwrap(),"scope":active.scope,"resource":resource});
        let rejected=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(chat)}));
        assert!(
            rejected.is_err(),
            "a Chat tag must never become a User sender plan: {rejected:?}"
        );
        assert!(harness.store.snapshot().unwrap().plans.is_empty());
        assert!(gateway.operation_log().await.is_empty());
        assert_eq!(gateway.chat_read_counts(), (0, 0));
        let plan=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(user)})).unwrap();
        assert_eq!(
            plan["recipe"]["payload"]["actor"],
            json!({"kind":"user","nativeId":"714"})
        );
        assert_eq!(harness.store.snapshot().unwrap().plans.len(), 1);
        assert!(gateway.operation_log().await.is_empty());
        let supported_chat = TelegramActorLocator::new(TelegramActorKind::Chat, "-1002")
            .unwrap()
            .resource(active.scope.account_id);
        // The negative Chat encoding is supported, but this gateway fixture has
        // no message authored by that chat. It must resolve as missing, not be
        // relabeled as a User or manufacture another executable plan.
        let missing=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(supported_chat)})).unwrap_err();
        assert_eq!(missing["code"], "not_found");
        assert_eq!(harness.store.snapshot().unwrap().plans.len(), 1);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn review_round1_intent_catalog_rejects_malformed_provider_output() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::providers::ports::IntentDescriptor;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        let request =
            json!({"actionId":"catalog","targets":[fixture()["messages"][2]["ref"]],"actor":null});
        let valid: Vec<IntentDescriptor> = serde_json::from_value(
            harness
                .call("get_intents_v2", &active, request.clone())
                .unwrap(),
        )
        .unwrap();
        let mut cases = vec![];
        let mut bad = valid.clone();
        bad[0].action_id = String::new();
        cases.push(bad);
        let mut bad = valid.clone();
        bad.push(bad[0].clone());
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].label = "bad\nlabel".into();
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors.clear();
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors[0].batch.max_targets = 0;
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors[0].availability = retract_domain::Availability::Unavailable;
        cases.push(bad);
        for (index, bad) in cases.into_iter().enumerate() {
            *io.intent_override.lock().unwrap() = Some(bad);
            assert_eq!(
                harness
                    .call("get_intents_v2", &active, request.clone())
                    .unwrap_err()["code"],
                "unsupported_schema",
                "case {index}"
            );
            assert!(harness.store.snapshot().unwrap().plans.is_empty());
            assert!(io.calls.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn review_round1_retry_preserves_the_native_absolute_deadline_through_latency_and_recovery() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use std::sync::atomic::Ordering;
        for recover in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let active = context("syntheticContext");
            let io = Arc::new(SyntheticIo::new(active.clone()));
            let deadline = chrono::Utc::now() + chrono::Duration::milliseconds(1650);
            *io.rate_deadline.lock().unwrap() = Some(deadline);
            io.rate_limit_once.store(true, Ordering::Release);
            io.preflight_delay_ms.store(150, Ordering::Release);
            let mut harness = synthetic(directory.path(), io.clone());
            let plan = harness.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
            harness.authorize(&active, &plan);
            let job = harness.start(&active, &plan).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if harness.store.snapshot().unwrap().jobs[0].retry_at.is_some() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                harness.store.snapshot().unwrap().jobs[0].retry_at,
                Some(deadline),
                "the native absolute deadline must not be rounded down or rebased"
            );
            if recover {
                let checkpoint = std::fs::read(directory.path().join("jobs.enc")).unwrap();
                harness.service.shutdown().await;
                let old = Arc::downgrade(&harness.store);
                drop(harness);
                assert!(old.upgrade().is_none());
                std::fs::write(directory.path().join("jobs.enc"), checkpoint).unwrap();
                *io.active.write().unwrap() = Some(active.clone());
                harness = synthetic(directory.path(), io.clone());
                invoke(
                    &harness.webview,
                    "get_bootstrap_snapshot_v2",
                    json!({"contractVersion":2,"context":active,"payload":{}}),
                )
                .unwrap();
            }
            while chrono::Utc::now() + chrono::Duration::milliseconds(30) < deadline {
                assert_eq!(io.preflights.lock().unwrap().len(), 1);
                assert!(io.calls.lock().unwrap().is_empty());
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let finished = harness.settled(&active, job.id).await;
            assert_eq!(finished.counters.deleted, 1);
            let preflights = io.preflights.lock().unwrap();
            assert_eq!(preflights.len(), 2);
            assert!(preflights[1] >= deadline);
            assert_eq!(io.calls.lock().unwrap().len(), 1);
        }
    });
}
