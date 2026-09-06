//! Task 1's eleven behavioral expectations, now exercising the real registered
//! v2 commands, shared lifecycle and authenticated v3 files. No native-ID shim.
use crate::{
    compatibility::{fixtures::*, tests::invoke},
    persistence::FoundationStore,
};
use retract_domain::*;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};

fn selected(other: bool) -> Vec<Value> {
    let data = fixture();
    let name = if other {
        "otherAccountMessages"
    } else {
        "messages"
    };
    vec![data[name][0]["ref"].clone(), data[name][1]["ref"].clone()]
}

#[test]
fn foundation_lifecycle_application_boundary_accepts_lossless_string_identifiers() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, _) = telegram(directory.path(), active.clone()).await;
        let plan = harness.prepare(&active, selected(false));
        let mut ids = plan
            .targets
            .iter()
            .map(|r| {
                r.resource.locator_payload["messageId"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["9007199254740992", "9007199254740993"]);
        assert!(
            plan.targets
                .iter()
                .all(|r| r.resource.locator_payload["chatId"] == "-1001")
        );
    });
}

#[test]
fn foundation_lifecycle_nonnumeric_target_survives_the_complete_shared_lifecycle() {
    tauri::async_runtime::block_on(async {
        let data = fixture();
        let active = context("syntheticContext");
        let reference = data["messages"][2]["ref"].clone();
        let conversation = data["messages"][2]["conversation"].clone();
        assert_eq!(
            reference["resource"]["locatorPayload"]["messageId"],
            "message:part/0007"
        );
        let directory = tempfile::tempdir().unwrap();
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let original = synthetic(directory.path(), io.clone());
        let plan = original.prepare(&active, vec![reference.clone()]);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(
            serde_json::to_value(&plan.scope).unwrap(),
            data["syntheticContext"]["scope"]
        );
        let prepared = encrypted_state(directory.path(), &active, KEY).unwrap();
        let prepared_plan = prepared.plans.iter().find(|p| p.id == plan.id).unwrap();
        assert_eq!(
            serde_json::to_value(&prepared_plan.targets).unwrap(),
            json!([reference])
        );
        assert_eq!(
            prepared_plan.targets[0].resource.locator_payload["messageId"],
            "message:part/0007"
        );
        original.authorize(&active, &plan);
        let job = original.start(&active, &plan).unwrap();
        assert_eq!(job.plan_id, plan.id);
        assert_eq!(job.counters.eligible, 1);
        assert_eq!(job.scope, active.scope);
        assert_eq!(
            serde_json::to_value(&job.dirty_refs).unwrap(),
            json!([conversation])
        );
        let finished = original.settled(&active, job.id).await;
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.counters.deleted, 1);
        let durable = encrypted_state(directory.path(), &active, KEY).unwrap();
        let ciphertext = std::fs::read(directory.path().join("jobs.enc")).unwrap();
        assert!(ciphertext.starts_with(b"RTRCT03"));
        assert!(
            !ciphertext
                .windows(b"message:part/0007".len())
                .any(|p| p == b"message:part/0007")
        );
        assert!(encrypted_state(directory.path(), &active, [0x72; 32]).is_err());
        let durable_plan = durable.plans.iter().find(|p| p.id == plan.id).unwrap();
        assert_eq!(durable_plan.scope, active.scope);
        assert_eq!(
            serde_json::to_value(&durable_plan.targets).unwrap(),
            json!([reference])
        );
        assert_eq!(
            durable_plan.targets[0].resource.locator_payload["messageId"],
            "message:part/0007"
        );
        while original.service.has_workers().await {
            tokio::task::yield_now().await;
        }
        let old_store = Arc::downgrade(&original.store);
        drop(original);
        assert!(
            old_store.upgrade().is_none(),
            "recovery must reopen ciphertext, not reuse an in-memory store"
        );
        let before = io.calls.lock().unwrap().clone();
        let reloaded = synthetic(directory.path(), io.clone());
        invoke(
            &reloaded.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":active,"payload":{}}),
        )
        .unwrap();
        let recovered = reloaded.settled(&active, job.id).await;
        assert_eq!(recovered.status, JobStatus::Completed);
        assert_eq!(recovered.counters.deleted, 1);
        assert_eq!(*io.calls.lock().unwrap(), before);
        assert_eq!(recovered.scope, active.scope);
        assert_eq!(
            serde_json::to_value(&recovered.dirty_refs).unwrap(),
            json!([conversation])
        );
        let refreshed = reloaded
            .call(
                "refresh_chats_v2",
                &active,
                json!({"conversations":recovered.dirty_refs}),
            )
            .unwrap();
        assert_eq!(refreshed.as_array().unwrap().len(), 1);
        let refreshed_ref = json!({"scope":refreshed[0]["scope"],"id":refreshed[0]["id"],"resource":refreshed[0]["resource"]});
        assert_eq!(refreshed_ref, conversation);
        assert_eq!(io.catalog.load(Ordering::SeqCst), 0);
        assert_eq!(io.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(before.len(), 1);
        assert_eq!(
            before[0][0].resource.locator_payload["messageId"],
            "message:part/0007"
        );
        let after = encrypted_state(directory.path(), &active, KEY).unwrap();
        assert_eq!(
            after
                .plans
                .iter()
                .find(|p| p.id == plan.id)
                .unwrap()
                .targets[0]
                .resource
                .locator_payload["messageId"],
            "message:part/0007"
        );
    });
}

#[test]
fn foundation_lifecycle_account_changes_invalidate_the_plan_binding() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (a, _) = telegram(directory.path(), context("context")).await;
        let first = a.prepare(&context("context"), selected(false));
        drop(a);
        let (b, _) = telegram(directory.path(), context("otherContext")).await;
        let other = b.prepare(&context("otherContext"), selected(true));
        assert_ne!(
            first.fingerprint, other.fingerprint,
            "same native targets in another account must not reuse the plan binding"
        );
    });
}

#[test]
fn foundation_lifecycle_changed_persisted_scope_rejects_the_original_fingerprint() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (original, _) = telegram(directory.path(), active.clone()).await;
        let plan = original.prepare(&active, selected(false));
        let mut changed = encrypted_state(directory.path(), &active, KEY).unwrap();
        changed.plans[0].scope = context("otherContext").scope;
        assert_eq!(changed.plans[0].id, plan.id);
        assert_eq!(changed.plans[0].fingerprint, plan.fingerprint);
        let aad = "{\"format\":\"RTRCT03\",\"schema\":3,\"provider\":\"telegram\",\"profile\":\"lifecycle\",\"scope\":\"profile\"}";
        let ciphertext = crate::secure_store::encrypt_authenticated(
            b"RTRCT03",
            &KEY,
            aad.as_bytes(),
            &serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        drop(original);
        std::fs::write(directory.path().join("jobs.enc"), ciphertext).unwrap();
        let result = FoundationStore::open_with_test_key_and_payload_validator(
            directory.path().to_path_buf(),
            binding(&active),
            KEY,
            Arc::new(crate::providers::telegram::locators::TelegramPayloadValidator),
        );
        assert!(
            matches!(result,Err(crate::error::AppError::SecureStore(ref message)) if message=="invalid remediation plan"),
            "scope tamper must fail the authenticated plan-binding validator: {result:?}"
        );
    });
}

#[test]
fn foundation_lifecycle_prepared_plan_persists_scoped_opaque_targets() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, _) = telegram(directory.path(), active.clone()).await;
        let plan = harness.prepare(&active, selected(false));
        let state = encrypted_state(directory.path(), &active, KEY).unwrap();
        let stored = state.plans.iter().find(|p| p.id == plan.id).unwrap();
        let mut ids = stored
            .targets
            .iter()
            .map(|r| r.resource.locator_payload["messageId"].as_str().unwrap())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["9007199254740992", "9007199254740993"]);
        assert_eq!(stored.scope, active.scope);
        let mut expected: Vec<ScopedResourceRef> = selected(false)
            .into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect();
        expected.sort_by_key(|r| r.id);
        assert_eq!(stored.targets, expected);
    });
}

#[test]
fn foundation_lifecycle_encrypted_job_roundtrip_retains_scoped_dirty_refs() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let plan = harness.prepare(&active, selected(false));
        harness.authorize(&active, &plan);
        let job = harness.start(&active, &plan).unwrap();
        let finished = harness.settled(&active, job.id).await;
        assert_eq!(finished.counters.deleted, 2);
        assert_eq!(
            gateway.delete_calls().await,
            vec![(-1001, vec![9_007_199_254_740_992, 9_007_199_254_740_993])]
        );
        let state = encrypted_state(directory.path(), &active, KEY).unwrap();
        let ciphertext = std::fs::read(directory.path().join("jobs.enc")).unwrap();
        assert!(
            !ciphertext
                .windows(b"9007199254740993".len())
                .any(|p| p == b"9007199254740993")
        );
        assert!(encrypted_state(directory.path(), &active, [0x72; 32]).is_err());
        let stored = state.jobs.iter().find(|j| j.id == job.id).unwrap();
        assert_eq!(
            serde_json::to_value(&stored.dirty_refs).unwrap(),
            fixture()["dirtyRefs"]
        );
        assert_eq!(stored.scope, active.scope);
    });
}

#[test]
fn foundation_lifecycle_foreign_account_execution_never_mutates_gateway() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let plan = harness.prepare(&active, selected(false));
        harness.authorize(&active, &plan);
        let result = harness.start(&context("otherContext"), &plan);
        assert!(gateway.operation_log().await.is_empty());
        assert_eq!(result.unwrap_err()["code"], "scope_mismatch");
    });
}

#[test]
fn foundation_lifecycle_encrypted_recovery_requires_verified_scope_before_replay() {
    tauri::async_runtime::block_on(async {
        for status in [JobStatus::Running, JobStatus::Queued] {
            let directory = tempfile::tempdir().unwrap();
            let active = context("context");
            let (harness, _) = telegram(directory.path(), active.clone()).await;
            let plan = harness.prepare(&active, selected(false));
            let legacy =
                crate::providers::telegram::recipe::TelegramExecutionRecipe::validate_envelope(
                    &plan,
                )
                .unwrap();
            let mut job = crate::providers::telegram::normalize::normalize_job(
                &active.scope,
                &legacy,
                &crate::providers::telegram::model::JobRecord::new(&legacy),
                true,
            )
            .unwrap();
            job.status = status;
            if status == JobStatus::Queued {
                job.retry_at = Some(chrono::Utc::now() + chrono::Duration::milliseconds(1400));
            }
            let id = job.id;
            harness
                .store
                .transaction(|s| {
                    s.jobs.push(job.clone());
                    Ok(())
                })
                .unwrap();
            let ciphertext = std::fs::read(directory.path().join("jobs.enc")).unwrap();
            let old = Arc::downgrade(&harness.store);
            drop(harness);
            assert!(old.upgrade().is_none());
            let mut reconnected = active.clone();
            reconnected.session_generation = uuid::Uuid::new_v4();
            let pending = PendingTelegram::reopen(directory.path(), &reconnected).await;
            let reopened = Harness::new(pending.clone());
            for _ in 0..2 {
                let snapshot = invoke(
                    &reopened.webview,
                    "get_bootstrap_snapshot_v2",
                    json!({"contractVersion":2,"context":null,"payload":{}}),
                )
                .unwrap();
                assert_eq!(snapshot["context"], Value::Null);
                assert_eq!(snapshot["payload"]["identity"]["state"], "pending");
                assert_eq!(pending.registrations.load(Ordering::Acquire), 0);
                assert!(pending.gateway.operation_log().await.is_empty());
                assert!(pending.gateway.current_reach_calls().await.is_empty());
                assert_eq!(pending.gateway.chat_read_counts(), (0, 0));
            }
            let state = encrypted_state(directory.path(), &active, KEY).unwrap();
            assert_eq!(
                state.jobs[0], job,
                "pending identity must preserve the entire checkpoint"
            );
            assert_eq!(
                std::fs::read(directory.path().join("jobs.enc")).unwrap(),
                ciphertext
            );
            assert_ne!(
                state.jobs.iter().find(|j| j.id == id).unwrap().status,
                JobStatus::Completed
            );
            assert_eq!(pending.verify(), reconnected);
            invoke(
                &reopened.webview,
                "get_bootstrap_snapshot_v2",
                json!({"contractVersion":2,"context":reconnected,"payload":{}}),
            )
            .unwrap();
            if let Some(deadline) = job.retry_at {
                while chrono::Utc::now() + chrono::Duration::milliseconds(40) < deadline {
                    assert!(pending.gateway.operation_log().await.is_empty());
                    assert!(pending.gateway.current_reach_calls().await.is_empty());
                    assert_eq!(reopened.store.snapshot().unwrap().jobs[0].next_batch, 0);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            let finished = reopened.settled(&reconnected, id).await;
            assert_eq!(finished.status, JobStatus::Completed);
            assert_eq!(finished.counters.deleted, 2);
            assert_eq!(pending.registrations.load(Ordering::Acquire), 1);
            assert_eq!(
                pending.gateway.delete_calls().await,
                vec![(-1001, vec![9_007_199_254_740_992, 9_007_199_254_740_993])]
            );
        }
    });
}

#[test]
fn foundation_lifecycle_targeted_refresh_returns_scoped_string_conversations() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let before = gateway.chat_read_counts();
        let conversation = fixture()["dirtyRefs"][0].clone();
        let chats = harness
            .call(
                "refresh_chats_v2",
                &active,
                json!({"conversations":[conversation,conversation]}),
            )
            .unwrap();
        assert_eq!(chats.as_array().unwrap().len(), 1);
        assert_eq!(gateway.chat_read_counts(), (before.0, before.1 + 1));
        assert_eq!(
            json!({"scope":chats[0]["scope"],"id":chats[0]["id"],"resource":chats[0]["resource"]}),
            conversation
        );
    });
}

#[test]
fn foundation_lifecycle_unknown_locator_version_is_rejected_before_resolution() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let mut reference = fixture()["messages"][0]["ref"].clone();
        reference["resource"]["locatorVersion"] = json!(999);
        let result = harness.call(
            "prepare_selection_v2",
            &active,
            json!({"messageRefs":[reference]}),
        );
        assert!(
            result.is_err(),
            "unsupported locator versions must not produce plans"
        );
        assert!(gateway.operation_log().await.is_empty());
        assert!(harness.store.snapshot().unwrap().plans.is_empty());
    });
}

#[test]
fn foundation_lifecycle_canonical_payload_disagreement_is_rejected() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let mut reference = fixture()["messages"][0]["ref"].clone();
        reference["resource"]["canonicalKey"] = json!("[\"-1001\",\"9007199254740993\"]");
        let result = harness.call(
            "prepare_selection_v2",
            &active,
            json!({"messageRefs":[reference]}),
        );
        assert!(
            result.is_err(),
            "canonical identity/locator payload disagreement must fail closed"
        );
        assert!(gateway.operation_log().await.is_empty());
        assert!(harness.store.snapshot().unwrap().plans.is_empty());
    });
}
