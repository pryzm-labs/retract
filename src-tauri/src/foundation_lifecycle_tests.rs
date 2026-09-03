//! Pre-migration RED gate: real legacy parser/service/AES-GCM store, synthetic I/O.
//! Task 6 must rewire these entry points to v2 without weakening the behavior.

use std::{path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    demo_gateway::DemoGateway,
    model::{
        AuthorizePlanRequest, ExecuteRequest, JobRecord, JobStatus, MessageRef, PersistedState,
        PlanView, PrepareSelectionRequest,
    },
    secure_store::SecureJobStore,
    service::CleanerService,
};

const KEY: [u8; 32] = [0x71; 32];

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../src/test/fixtures/provider-lifecycle.json"
    ))
    .expect("complete synthetic lifecycle fixture")
}

// Explicit temporary legacy adapter for the already numeric Telegram engine.
// These supplemental native fields let later lifecycle tests reach the engine
// independently of the RED string parser below. No f64/JavaScript conversion.
fn legacy_selection(context: Value, refs: Vec<Value>) -> PrepareSelectionRequest {
    let message_refs = refs
        .iter()
        .map(|reference| {
            let locator = &reference["resource"]["locatorPayload"];
            json!({
                "chatId": locator["chatId"].as_str().unwrap().parse::<i64>().unwrap(),
                "messageId": locator["messageId"].as_str().unwrap().parse::<i64>().unwrap(),
                "ref": reference
            })
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "contractVersion": 2, "context": context,
        "payload": { "messageRefs": refs }, "messageRefs": message_refs
    }))
    .unwrap()
}

async fn service(path: &Path) -> (Arc<DemoGateway>, Arc<CleanerService>) {
    let gateway = Arc::new(DemoGateway::new());
    gateway
        .append_messages(-1001, 9_007_199_254_740_992, 2)
        .await;
    let service = CleanerService::new(
        gateway.clone(),
        SecureJobStore::with_test_key(path.to_path_buf(), KEY),
    )
    .unwrap();
    (gateway, service)
}

async fn prepare(service: &CleanerService, context: Value) -> PlanView {
    let data = fixture();
    let messages = if context["scope"] == data["otherScope"] {
        &data["otherAccountMessages"]
    } else {
        &data["messages"]
    };
    service
        .prepare_selection(legacy_selection(
            context,
            vec![messages[0]["ref"].clone(), messages[1]["ref"].clone()],
        ))
        .await
        .expect("the real legacy engine can prepare the two exact i64 targets")
}

async fn authorize(service: &CleanerService, plan: &PlanView, context: Value) {
    let request: AuthorizePlanRequest = serde_json::from_value(json!({
        "contractVersion": 2, "context": context,
        "planId": plan.id, "fingerprint": plan.fingerprint,
        "payload": { "planId": plan.id, "fingerprint": plan.fingerprint }
    }))
    .unwrap();
    service.authorize_plan(request).await.unwrap();
}

fn execution(plan: &PlanView, context: Value) -> ExecuteRequest {
    let payload = json!({
        "planId": plan.id, "fingerprint": plan.fingerprint,
        "irreversibleAcknowledged": true, "typedChatTitle": null
    });
    let mut request = payload.clone();
    request["contractVersion"] = json!(2);
    request["context"] = context;
    request["payload"] = payload;
    serde_json::from_value(request).unwrap()
}

async fn settled(service: &CleanerService, job_id: Uuid) -> JobRecord {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let job = service
                .jobs()
                .await
                .into_iter()
                .find(|job| job.id == job_id)
                .unwrap();
            let wire = serde_json::to_value(&job).unwrap();
            if job.status.is_terminal() || wire["status"] == "blocked" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("synthetic job reaches a terminal or scope-blocked state")
}

async fn persisted(path: &Path, job_id: Uuid) -> PersistedState {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state = SecureJobStore::with_test_key(path.to_path_buf(), KEY)
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
    .expect("terminal job was actually encrypted and saved")
}

#[test]
fn foundation_lifecycle_application_boundary_accepts_lossless_string_identifiers() {
    let parsed = serde_json::from_value::<MessageRef>(json!({
        "chatId": "-1001", "messageId": "9007199254740993"
    }));
    assert!(
        parsed.is_ok(),
        "the application boundary must accept lossless string identifiers: {parsed:?}"
    );
}

#[test]
fn foundation_lifecycle_nonnumeric_target_survives_the_complete_shared_lifecycle() {
    tauri::async_runtime::block_on(async {
        let data = fixture();
        let context = data["syntheticContext"].clone();
        let reference = data["messages"][2]["ref"].clone();
        let conversation = data["messages"][2]["conversation"].clone();
        assert_eq!(
            reference["resource"]["locatorPayload"]["messageId"],
            "message:part/0007"
        );

        // R4: one scenario owns every downstream assertion. It is intentionally
        // blocked at this real parser today, not claimed as end-to-end GREEN.
        // Tasks 4/6 replace the boundaries with the shared registered synthetic
        // provider path. Never route this target through legacy_selection or
        // substitute an i64 for its opaque native ID to reach the next step.
        let request = serde_json::from_value::<PrepareSelectionRequest>(json!({
            "contractVersion": 2, "context": context,
            "payload": { "messageRefs": [reference] },
            "messageRefs": [{ "chatId": -1001, "messageId": "message:part/0007" }]
        }))
        .expect("the full shared lifecycle must accept the literal nonnumeric target at its application boundary");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let gateway = Arc::new(DemoGateway::new());
        let original = CleanerService::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), KEY),
        )
        .expect("open the real encrypted service for the synthetic lifecycle");
        let plan = original
            .prepare_selection(request)
            .await
            .expect("prepare the exact opaque target through the shared service");
        assert_eq!(plan.summary.selected, 1);
        assert_eq!(
            serde_json::to_value(&plan).unwrap()["scope"],
            context["scope"]
        );

        let prepared_state = SecureJobStore::with_test_key(path.clone(), KEY)
            .load()
            .expect("preparation must durably save the frozen opaque plan");
        let prepared_plan = prepared_state
            .plans
            .iter()
            .find(|stored| stored.id == plan.id)
            .unwrap();
        let prepared_wire = serde_json::to_value(prepared_plan).unwrap();
        assert_eq!(prepared_wire["targets"], json!([reference]));
        assert_eq!(
            prepared_wire["targets"][0]["resource"]["locatorPayload"]["messageId"],
            "message:part/0007"
        );

        // Existing real authorization/execution, not a test-only provider engine.
        authorize(&original, &plan, context.clone()).await;
        let job = original
            .start_execution(execution(&plan, context.clone()))
            .await
            .expect("the authorization must start this exact scoped frozen plan");
        assert_eq!(job.plan_id, plan.id);
        assert_eq!(job.total, 1);
        let job_wire = serde_json::to_value(&job).unwrap();
        assert_eq!(job_wire["scope"], context["scope"]);
        assert_eq!(job_wire["dirtyRefs"], json!([conversation]));
        let finished = settled(&original, job.id).await;
        assert_eq!(finished.status, JobStatus::Completed);
        assert_eq!(finished.deleted, 1);

        let durable = persisted(&path, job.id).await;
        let ciphertext = std::fs::read(&path).expect("read the actual encrypted file");
        assert!(ciphertext.starts_with(b"RTRCT03"));
        assert!(
            !ciphertext
                .windows(b"message:part/0007".len())
                .any(|bytes| bytes == b"message:part/0007")
        );
        assert!(
            SecureJobStore::with_test_key(path.clone(), [0x72; 32])
                .load()
                .is_err()
        );
        let durable_plan = durable
            .plans
            .iter()
            .find(|stored| stored.id == plan.id)
            .unwrap();
        let durable_wire = serde_json::to_value(durable_plan).unwrap();
        assert_eq!(durable_wire["scope"], context["scope"]);
        assert_eq!(durable_wire["targets"], json!([reference]));
        assert_eq!(
            durable_wire["targets"][0]["resource"]["locatorPayload"]["messageId"],
            "message:part/0007"
        );

        drop(original);
        let reloaded = CleanerService::new(
            gateway.clone(),
            SecureJobStore::with_test_key(path.clone(), KEY),
        )
        .expect("recover the real service from the authenticated scoped job file");
        let mutations_before_recovery = gateway.operation_log().await;
        reloaded.resume_incomplete().await;
        let recovered = settled(&reloaded, job.id).await;
        assert_eq!(recovered.status, JobStatus::Completed);
        assert_eq!(recovered.deleted, 1);
        assert_eq!(gateway.operation_log().await, mutations_before_recovery);
        let recovered_wire = serde_json::to_value(&recovered).unwrap();
        assert_eq!(recovered_wire["scope"], context["scope"]);
        assert_eq!(recovered_wire["dirtyRefs"], json!([conversation]));

        // This is the existing refresh parser/service signature, another boundary
        // to rewire to v2. Return the stored scoped ref unchanged; do not extract
        // or parse its native chat ID merely to satisfy the legacy Vec<i64> API.
        let refresh_refs = serde_json::from_value(recovered_wire["dirtyRefs"].clone())
            .expect("targeted refresh must accept the recovered scoped conversation refs");
        let refreshed = reloaded.refresh_chats(refresh_refs).await.unwrap();
        assert_eq!(refreshed.len(), 1);
        assert_eq!(
            serde_json::to_value(&refreshed[0]).unwrap()["ref"],
            conversation
        );
        let after_refresh = SecureJobStore::with_test_key(path, KEY).load().unwrap();
        let retained = after_refresh
            .plans
            .iter()
            .find(|stored| stored.id == plan.id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(retained).unwrap()["targets"][0]["resource"]["locatorPayload"]["messageId"],
            "message:part/0007"
        );
    });
}

#[test]
fn foundation_lifecycle_account_changes_invalidate_the_plan_binding() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (_, service) = service(&directory.path().join("jobs.enc")).await;
        let data = fixture();
        let first = prepare(&service, data["context"].clone()).await;
        let other = prepare(&service, data["otherContext"].clone()).await;
        assert_ne!(
            first.fingerprint, other.fingerprint,
            "the same native targets in another account must not reuse an unscoped plan binding"
        );
    });
}

#[test]
fn foundation_lifecycle_changed_persisted_scope_rejects_the_original_fingerprint() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let (gateway, original) = service(&path).await;
        let data = fixture();
        let plan = prepare(&original, data["context"].clone()).await;
        let store = SecureJobStore::with_test_key(path.clone(), KEY);
        let mut wire = serde_json::to_value(store.load().unwrap()).unwrap();
        wire["plans"][0]["scope"] = data["otherScope"].clone();
        // Keep ID, targets and fingerprint unchanged: a new random plan ID must
        // not let the cross-account comparison pass without actually binding scope.
        let changed = serde_json::from_value::<PersistedState>(wire)
            .expect("scope tampering must not be mistaken for an invalid test fixture");
        store.save(&changed).unwrap();
        drop(original);
        // A future v2 loader may reject the binding here, but that adaptation
        // must assert its specific safe binding-error code, not accept any error.
        let reloaded = CleanerService::new(gateway, SecureJobStore::with_test_key(path, KEY))
            .expect("the current legacy store must load before checking authorization rejection");
        let request = serde_json::from_value(json!({
            "contractVersion": 2, "context": data["context"],
            "planId": plan.id, "fingerprint": plan.fingerprint,
            "payload": { "planId": plan.id, "fingerprint": plan.fingerprint }
        }))
        .unwrap();
        assert!(
            reloaded.authorize_plan(request).await.is_err(),
            "changing only persisted account scope must invalidate the original plan fingerprint"
        );
    });
}

#[test]
fn foundation_lifecycle_prepared_plan_persists_scoped_opaque_targets() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let (_, service) = service(&path).await;
        let data = fixture();
        let plan = prepare(&service, data["context"].clone()).await;
        let state = SecureJobStore::with_test_key(path, KEY).load().unwrap();
        let stored = state
            .plans
            .iter()
            .find(|candidate| candidate.id == plan.id)
            .unwrap();
        assert_eq!(
            stored
                .items
                .iter()
                .map(|item| item.message_id)
                .collect::<Vec<_>>(),
            vec![9_007_199_254_740_992, 9_007_199_254_740_993]
        );
        let wire = serde_json::to_value(stored).unwrap();
        assert_eq!(
            wire["scope"], data["context"]["scope"],
            "encrypted frozen plans must bind the provider/account/source, not only numeric native targets"
        );
        assert_eq!(
            wire["targets"],
            json!([data["messages"][0]["ref"], data["messages"][1]["ref"]])
        );
    });
}

#[test]
fn foundation_lifecycle_encrypted_job_roundtrip_retains_scoped_dirty_refs() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let (gateway, service) = service(&path).await;
        let data = fixture();
        let plan = prepare(&service, data["context"].clone()).await;
        authorize(&service, &plan, data["context"].clone()).await;
        let job = service
            .start_execution(execution(&plan, data["context"].clone()))
            .await
            .unwrap();
        let finished = settled(&service, job.id).await;
        assert_eq!(finished.deleted, 2);
        assert_eq!(
            gateway.delete_calls().await,
            vec![(-1001, vec![9_007_199_254_740_992, 9_007_199_254_740_993])]
        );
        let state = persisted(&path, job.id).await;
        let ciphertext = std::fs::read(&path).unwrap();
        assert!(
            !ciphertext
                .windows(b"9007199254740993".len())
                .any(|part| part == b"9007199254740993")
        );
        assert!(
            SecureJobStore::with_test_key(path.clone(), [0x72; 32])
                .load()
                .is_err()
        );
        let wire = serde_json::to_value(
            state
                .jobs
                .iter()
                .find(|candidate| candidate.id == job.id)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            wire["dirtyRefs"], data["dirtyRefs"],
            "an actual AES-GCM save/reload must retain scoped dirty conversation refs"
        );
        assert_eq!(wire["scope"], data["context"]["scope"]);
    });
}

#[test]
fn foundation_lifecycle_foreign_account_execution_never_mutates_gateway() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (gateway, service) = service(&directory.path().join("jobs.enc")).await;
        let data = fixture();
        let plan = prepare(&service, data["context"].clone()).await;
        authorize(&service, &plan, data["context"].clone()).await;
        let result = service
            .start_execution(execution(&plan, data["otherContext"].clone()))
            .await;
        if let Ok(job) = &result {
            settled(&service, job.id).await;
        }
        assert!(
            gateway.operation_log().await.is_empty(),
            "a foreign-account request must not reach a mutation; observed {:?}",
            gateway.operation_log().await
        );
        assert!(
            result.is_err(),
            "execution must fail closed on expected-context mismatch"
        );
    });
}

#[test]
fn foundation_lifecycle_encrypted_recovery_requires_verified_scope_before_replay() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let (gateway, preparation) = service(&path).await;
        let data = fixture();
        let view = prepare(&preparation, data["context"].clone()).await;
        let store = SecureJobStore::with_test_key(path.clone(), KEY);
        let mut state = store.load().unwrap();
        let plan = state.plans.iter().find(|plan| plan.id == view.id).unwrap();
        let mut job = JobRecord::new(plan);
        job.status = JobStatus::Running;
        let job_id = job.id;
        state.jobs.push(job);
        store.save(&state).unwrap();
        drop(preparation);
        // The new runtime has authenticated ciphertext but no verified account.
        // That is not authority to attach this queued legacy job to its session.
        let resumed =
            CleanerService::new(gateway.clone(), SecureJobStore::with_test_key(path, KEY)).unwrap();
        resumed.resume_incomplete().await;
        let finished = settled(&resumed, job_id).await;
        assert!(
            gateway.operation_log().await.is_empty(),
            "encrypted recovery without verified scope must not replay native deletions: {:?}",
            gateway.operation_log().await
        );
        assert_ne!(finished.status, JobStatus::Completed);
    });
}

#[test]
fn foundation_lifecycle_targeted_refresh_returns_scoped_string_conversations() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (gateway, service) = service(&directory.path().join("jobs.enc")).await;
        let before = gateway.chat_read_counts();
        let chats = service.refresh_chats(vec![-1001, -1001]).await.unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(gateway.chat_read_counts(), (before.0, before.1 + 1));
        let wire = serde_json::to_value(&chats[0]).unwrap();
        assert_eq!(
            wire["ref"],
            fixture()["dirtyRefs"][0],
            "targeted reconciliation must return the scoped opaque conversation ref, not a naked native chat ID"
        );
    });
}

#[test]
fn foundation_lifecycle_unknown_locator_version_is_rejected_before_resolution() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (_, service) = service(&directory.path().join("jobs.enc")).await;
        let data = fixture();
        let mut reference = data["messages"][0]["ref"].clone();
        reference["resource"]["locatorVersion"] = json!(999);
        let result = service
            .prepare_selection(legacy_selection(data["context"].clone(), vec![reference]))
            .await;
        assert!(
            result.is_err(),
            "unsupported locator versions must not produce executable plans"
        );
    });
}

#[test]
fn foundation_lifecycle_canonical_payload_disagreement_is_rejected() {
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (_, service) = service(&directory.path().join("jobs.enc")).await;
        let data = fixture();
        let mut reference = data["messages"][0]["ref"].clone();
        reference["resource"]["canonicalKey"] = json!("[\"-1001\",\"9007199254740993\"]");
        let result = service
            .prepare_selection(legacy_selection(data["context"].clone(), vec![reference]))
            .await;
        assert!(
            result.is_err(),
            "canonical identity and locator payload disagreement must fail closed"
        );
    });
}
