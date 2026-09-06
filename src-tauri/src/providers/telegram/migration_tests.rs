use std::{collections::BTreeSet, fs, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use cleaner_domain::PlanOperation;
use retract_domain::{ErrorCode, ExpectedEffect, JobStatus, RemediationPlan};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    locators::{TelegramMessageLocator, TelegramPayloadValidator, telegram_provider_key},
    recipe::{EXECUTION_SCHEMA, TelegramExecutionRecipe, bind_plan},
};
use crate::persistence::{FoundationStore, StoreBinding};

const BASE_COMMIT: &str = "c2300593fcc0b94c710d6a535173799dd36da4f0";
const STORE_KEY: [u8; 32] = [0x54; 32];
const FIXTURES: [(&str, &str); 9] = [
    (
        "selected.json",
        include_str!("../../../test-fixtures/telegram-migration/selected.json"),
    ),
    (
        "delete-my-messages.json",
        include_str!("../../../test-fixtures/telegram-migration/delete-my-messages.json"),
    ),
    (
        "clear-history.json",
        include_str!("../../../test-fixtures/telegram-migration/clear-history.json"),
    ),
    (
        "clear-history-and-leave.json",
        include_str!("../../../test-fixtures/telegram-migration/clear-history-and-leave.json"),
    ),
    (
        "delete-all-messages-and-leave.json",
        include_str!(
            "../../../test-fixtures/telegram-migration/delete-all-messages-and-leave.json"
        ),
    ),
    (
        "remove-chat-for-self.json",
        include_str!("../../../test-fixtures/telegram-migration/remove-chat-for-self.json"),
    ),
    (
        "delete-by-sender.json",
        include_str!("../../../test-fixtures/telegram-migration/delete-by-sender.json"),
    ),
    (
        "delete-group.json",
        include_str!("../../../test-fixtures/telegram-migration/delete-group.json"),
    ),
    (
        "leave-chat.json",
        include_str!("../../../test-fixtures/telegram-migration/leave-chat.json"),
    ),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    artifact_version: u16,
    base_commit: String,
    execution_schema: String,
    execution_schema_version: u16,
    store: StoreManifest,
    entries: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreManifest {
    fixture: String,
    magic: String,
    profile: String,
    test_key: String,
    byte_length: usize,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEntry {
    fixture: String,
    operation: PlanOperation,
    plan_id: Uuid,
    job_id: Uuid,
    scope: String,
    fingerprint: String,
    resumable: bool,
    started_authorized: bool,
}

fn manifest() -> Manifest {
    serde_json::from_str(include_str!(
        "../../../test-fixtures/telegram-migration/manifest.json"
    ))
    .unwrap()
}

fn plans() -> Vec<RemediationPlan> {
    FIXTURES
        .iter()
        .map(|(_, fixture)| serde_json::from_str(fixture).unwrap())
        .collect()
}

fn store_bytes() -> Vec<u8> {
    let encoded = include_str!("../../../test-fixtures/telegram-migration/store-v3.b64")
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    STANDARD.decode(encoded).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn store_binding(manifest: &Manifest) -> StoreBinding {
    StoreBinding {
        provider: telegram_provider_key(),
        profile: manifest.store.profile.clone(),
    }
}

#[test]
fn frozen_base_envelopes_validate_rebind_and_retain_exact_fingerprints() {
    let manifest = manifest();
    assert_eq!(manifest.artifact_version, 1);
    assert_eq!(manifest.base_commit, BASE_COMMIT);
    assert_eq!(manifest.execution_schema, EXECUTION_SCHEMA);
    assert_eq!(manifest.execution_schema_version, 1);
    assert_eq!(manifest.entries.len(), FIXTURES.len());

    let expected_operations = [
        PlanOperation::SelectedMessages,
        PlanOperation::DeleteMyMessages,
        PlanOperation::ClearHistory,
        PlanOperation::ClearHistoryAndLeave,
        PlanOperation::DeleteAllMessagesAndLeave,
        PlanOperation::RemoveChatForSelf,
        PlanOperation::DeleteBySender,
        PlanOperation::DeleteGroup,
        PlanOperation::LeaveChat,
    ]
    .into_iter()
    .map(|operation| serde_json::to_string(&operation).unwrap())
    .collect::<BTreeSet<_>>();
    let mut observed_operations = BTreeSet::new();
    let foreign_scope = serde_json::from_str::<RemediationPlan>(FIXTURES[8].1)
        .unwrap()
        .scope;

    for ((fixture_name, fixture), entry) in FIXTURES.iter().zip(&manifest.entries) {
        let expected: RemediationPlan = serde_json::from_str(fixture).unwrap();
        assert_eq!(*fixture_name, entry.fixture);
        assert_eq!(expected.id, entry.plan_id);
        assert_eq!(expected.fingerprint, entry.fingerprint);
        assert_eq!(expected.recipe.schema, manifest.execution_schema);
        assert_eq!(expected.recipe.version, manifest.execution_schema_version);
        assert_eq!(expected.scope == foreign_scope, entry.scope == "foreign");

        let mut native = TelegramExecutionRecipe::validate_envelope(&expected).unwrap();
        assert_eq!(native.id, expected.id);
        assert_eq!(native.operation, entry.operation);
        assert_eq!(native.fingerprint, expected.fingerprint);
        let rebound = bind_plan(&expected.scope, &mut native).unwrap();
        assert_eq!(
            rebound, expected,
            "base envelope changed for {fixture_name}"
        );
        observed_operations.insert(serde_json::to_string(&entry.operation).unwrap());
    }

    assert_eq!(observed_operations, expected_operations);
}

#[test]
fn frozen_effect_locator_and_recipe_version_mutations_are_rejected() {
    let expected: RemediationPlan = serde_json::from_str(FIXTURES[0].1).unwrap();

    let mut changed_version = expected.clone();
    changed_version.recipe.version = 2;
    changed_version.seal().unwrap();
    assert!(TelegramExecutionRecipe::validate_envelope(&changed_version).is_err());

    let mut changed_effect = expected.clone();
    changed_effect.steps[0].descriptor.effect = ExpectedEffect::RemovedForCurrentAccountOnly;
    changed_effect.seal().unwrap();
    assert!(TelegramExecutionRecipe::validate_envelope(&changed_effect).is_err());

    let mut changed_locator = expected.clone();
    let replacement = TelegramMessageLocator::new("-1001", "9007199254740994")
        .unwrap()
        .scoped(expected.scope.clone());
    changed_locator.steps[0].targets[0] = replacement.clone();
    changed_locator.targets[0] = replacement;
    changed_locator.seal().unwrap();
    assert!(TelegramExecutionRecipe::validate_envelope(&changed_locator).is_err());
}

#[test]
fn frozen_authenticated_v3_bytes_load_without_rewrite_and_retain_recovery_decisions() {
    let manifest = manifest();
    let bytes = store_bytes();
    assert_eq!(manifest.store.fixture, "store-v3.b64");
    assert_eq!(manifest.store.magic, "RTRCT03");
    assert_eq!(manifest.store.test_key, "32 bytes repeated 0x54");
    assert_eq!(bytes.len(), manifest.store.byte_length);
    assert_eq!(sha256(&bytes), manifest.store.sha256);
    assert!(bytes.starts_with(b"RTRCT03"));

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("jobs.enc");
    fs::write(&path, &bytes).unwrap();
    let store = FoundationStore::open_with_test_key_and_payload_validator(
        directory.path().to_path_buf(),
        store_binding(&manifest),
        STORE_KEY,
        Arc::new(TelegramPayloadValidator),
    )
    .unwrap();
    let state = store.snapshot().unwrap();
    assert_eq!(state.identities.len(), 2);
    assert_eq!(state.sources.len(), 2);
    assert_eq!(state.plans, plans());
    assert_eq!(state.jobs.len(), manifest.entries.len());
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "valid v3 load rewrote bytes"
    );

    for entry in &manifest.entries {
        let plan = state
            .plans
            .iter()
            .find(|plan| plan.id == entry.plan_id)
            .unwrap();
        let job = state
            .jobs
            .iter()
            .find(|job| job.id == entry.job_id)
            .unwrap();
        assert_eq!(job.plan_id, plan.id);
        assert_eq!(job.started_authorized, entry.started_authorized);
        assert_eq!(
            crate::providers::lifecycle::resumable(plan, job),
            entry.resumable,
            "recovery decision changed for {}",
            entry.fixture
        );
        assert_eq!(
            (job.scope != state.plans[0].scope),
            entry.scope == "foreign"
        );
    }

    let retry_job = state
        .jobs
        .iter()
        .find(|job| job.id == manifest.entries[0].job_id)
        .unwrap();
    let retry_at = "2026-09-06T12:07:00Z".parse().unwrap();
    assert_eq!(retry_job.retry_at, Some(retry_at));
    assert_eq!(retry_job.diagnostics.len(), 1);
    assert_eq!(retry_job.diagnostics[0].code, ErrorCode::RateLimited);
    assert_eq!(retry_job.diagnostics[0].retry_at, Some(retry_at));

    crate::providers::lifecycle::block_foreign_jobs(&store, &state.plans[0].scope).unwrap();
    let recovered = store.snapshot().unwrap();
    let foreign = recovered
        .jobs
        .iter()
        .find(|job| job.id == manifest.entries[8].job_id)
        .unwrap();
    assert_eq!(foreign.status, JobStatus::Blocked);
    assert!(
        foreign
            .diagnostics
            .iter()
            .any(|error| { error.code == ErrorCode::ScopeMismatch && error.retry_at.is_none() })
    );
}

#[test]
fn frozen_v3_ciphertext_corruption_and_wrong_key_fail_closed() {
    for (bytes, key) in [
        (store_bytes(), [0x55; 32]),
        (
            {
                let mut bytes = store_bytes();
                let last = bytes.len() - 1;
                bytes[last] ^= 1;
                bytes
            },
            STORE_KEY,
        ),
    ] {
        let manifest = manifest();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, &bytes).unwrap();
        assert!(
            FoundationStore::open_with_test_key_and_payload_validator(
                directory.path().to_path_buf(),
                store_binding(&manifest),
                key,
                Arc::new(TelegramPayloadValidator),
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
