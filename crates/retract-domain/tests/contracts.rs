use retract_domain::*;
use serde_json::{Value, json};
use uuid::Uuid;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../src/test/fixtures/provider-lifecycle.json"
    ))
    .unwrap()
}
fn target(index: usize) -> ScopedResourceRef {
    serde_json::from_value(fixture()["messages"][index]["ref"].clone()).unwrap()
}
fn scope() -> Scope {
    serde_json::from_value(fixture()["context"]["scope"].clone()).unwrap()
}
fn plan() -> RemediationPlan {
    let targets = vec![target(0), target(1)];
    let mut plan: RemediationPlan = serde_json::from_value(json!({
        "id":"ffffffff-ffff-4fff-8fff-ffffffffffff", "scope":scope(),
        "steps":[{"descriptor":{
            "id":"delete-selected", "kind":"delete_remote_item",
            "effect":"removed_for_all_participants", "availability":"executable",
            "unavailableReason":null, "requiresLivePreflight":true,
            "batch":{"maxTargets":100,"maxParallel":1}, "confirmationTier":"low",
            "destructive":true, "irreversible":true, "advisory":null
        },"targets":targets}], "targets":targets,
        "confirmation":{"tier":"low", "acknowledgementRequired":true,
            "ownerAuthRequired":true,"exactText":null},
        "recipe":{"schema":"synthetic.frozen","version":1,
            "payload":{"operation":"selected","limit":100}},
        "restartPolicy":"resume_frozen_targets", "createdAt":"2026-09-03T00:00:00Z",
        "fingerprint":""
    }))
    .unwrap();
    plan.seal().unwrap();
    plan
}

#[test]
fn provider_keys_reject_untrusted_or_unbounded_names_at_deserialization() {
    for invalid in [
        "",
        "Telegram",
        "1test",
        "a.b",
        "é",
        "a b",
        "a/",
        &"a".repeat(65),
    ] {
        assert!(
            serde_json::from_value::<ProviderKey>(json!(invalid)).is_err(),
            "{invalid}"
        );
    }
    for valid in ["telegram", "synthetic_2-x", &"a".repeat(64)] {
        let parsed: ProviderKey = serde_json::from_value(json!(valid)).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(valid));
    }
}

#[test]
fn uuid_ids_are_strings_and_reject_nil_or_numeric_input() {
    let account: AccountId =
        serde_json::from_value(json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")).unwrap();
    assert_eq!(
        serde_json::to_value(account).unwrap(),
        json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
    );
    assert!(serde_json::from_value::<AccountId>(json!(42)).is_err());
    assert!(serde_json::from_value::<AccountId>(json!(Uuid::nil())).is_err());
    let mut context: ActiveContext = serde_json::from_value(fixture()["context"].clone()).unwrap();
    context.session_generation = Uuid::nil();
    assert!(context.validate().is_err());
}

#[test]
fn standard_uuid_v5_preserves_large_and_nonnumeric_native_strings() {
    let records = fixture();
    for item in records["messages"]
        .as_array()
        .unwrap()
        .iter()
        .chain(records["chats"].as_array().unwrap())
    {
        let reference: ScopedResourceRef = serde_json::from_value(item["ref"].clone()).unwrap();
        assert_eq!(
            reference.resource.resource_id().unwrap().to_string(),
            item["ref"]["id"].as_str().unwrap()
        );
        reference.validate(&reference.scope).unwrap();
    }
    assert_eq!(
        target(0).resource.locator_payload["messageId"],
        "9007199254740992"
    );
    assert_eq!(
        target(1).resource.locator_payload["messageId"],
        "9007199254740993"
    );
    assert_ne!(
        target(0).resource.resource_id().unwrap(),
        target(1).resource.resource_id().unwrap()
    );
}

#[test]
fn reference_validation_checks_scope_and_derived_identity() {
    let good = target(0);
    for path in ["/scope/provider", "/resource/provider"] {
        let mut value = serde_json::to_value(&good).unwrap();
        *value.pointer_mut(path).unwrap() = json!("synthetic");
        let bad: ScopedResourceRef = serde_json::from_value(value).unwrap();
        assert!(bad.validate(&scope()).is_err(), "{path}");
    }
    for path in [
        "/scope/accountId",
        "/resource/accountId",
        "/scope/sourceId",
        "/id",
    ] {
        let mut value = serde_json::to_value(&good).unwrap();
        *value.pointer_mut(path).unwrap() = json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        assert!(
            serde_json::from_value::<ScopedResourceRef>(value)
                .unwrap()
                .validate(&scope())
                .is_err(),
            "{path}"
        );
    }
    let mut other_source = good.clone();
    other_source.scope.source_id =
        serde_json::from_value(json!("22222222-2222-4222-8222-222222222222")).unwrap();
    assert_ne!(other_source.scope, good.scope);
    assert_eq!(other_source.resource.resource_id().unwrap(), good.id);
    assert!(other_source.validate(&scope()).is_err());
}

#[test]
fn reference_and_payload_envelopes_reject_invalid_bounds() {
    for (field, value) in [
        ("locatorSchema", json!("")),
        ("locatorSchema", json!("x".repeat(129))),
        ("locatorVersion", json!(0)),
        ("canonicalKey", json!("")),
        ("canonicalKey", json!("x".repeat(4097))),
    ] {
        let mut bad = serde_json::to_value(target(0).resource).unwrap();
        bad[field] = value;
        assert!(
            serde_json::from_value::<ProviderResourceRef>(bad)
                .unwrap()
                .resource_id()
                .is_err()
        );
    }
    let payload = VersionedPayload {
        schema: "".into(),
        version: 1,
        payload: json!({}),
    };
    assert!(payload.validate().is_err());
}

#[test]
fn sealed_plan_binds_every_immutable_field() {
    let original = plan();
    assert!(original.fingerprint.starts_with("sha256-v1:"));
    // Independent Node crypto SHA-256 over hand-built canonical JSON; no Rust
    // serializer or fingerprint helper generated this expected value.
    assert_eq!(
        original.fingerprint,
        "sha256-v1:515df8e9f662a144fa0861b98910151d5dd1c2ed346485b92be3249ba299d48d"
    );
    original.validate().unwrap();
    for (path, value) in [
        ("/id", json!("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee")),
        ("/scope/provider", json!("synthetic")),
        (
            "/scope/accountId",
            json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        ),
        (
            "/scope/sourceId",
            json!("22222222-2222-4222-8222-222222222222"),
        ),
        ("/steps/0/descriptor/id", json!("other")),
        ("/steps/0/descriptor/kind", json!("clear_conversation")),
        (
            "/steps/0/descriptor/effect",
            json!("removed_for_current_account_only"),
        ),
        ("/steps/0/descriptor/availability", json!("manual_only")),
        (
            "/steps/0/descriptor/unavailableReason",
            json!({"code":"permission_changed","retryAt":null}),
        ),
        ("/steps/0/descriptor/requiresLivePreflight", json!(false)),
        ("/steps/0/descriptor/batch/maxTargets", json!(50)),
        ("/steps/0/descriptor/batch/maxParallel", json!(2)),
        ("/steps/0/descriptor/confirmationTier", json!("medium")),
        ("/steps/0/descriptor/destructive", json!(false)),
        ("/steps/0/descriptor/irreversible", json!(false)),
        (
            "/steps/0/descriptor/advisory",
            json!({"costBearing":true,"rateLimited":false}),
        ),
        ("/confirmation/tier", json!("high")),
        ("/confirmation/acknowledgementRequired", json!(false)),
        ("/confirmation/ownerAuthRequired", json!(false)),
        ("/confirmation/exactText", json!("Exact synthetic title")),
        ("/recipe/schema", json!("synthetic.other")),
        ("/recipe/version", json!(2)),
        ("/recipe/payload/limit", json!(101)),
        ("/restartPolicy", json!("requires_new_review")),
        ("/createdAt", json!("2026-09-04T00:00:00Z")),
        (
            "/targets/0/resource/locatorSchema",
            json!("synthetic.message"),
        ),
        ("/targets/0/resource/locatorVersion", json!(2)),
        ("/targets/0/resource/canonicalKey", json!("other")),
        (
            "/targets/0/resource/locatorPayload/messageId",
            json!("message:part/0007"),
        ),
        ("/fingerprint", json!("sha256-v1:bad")),
    ] {
        let mut changed = serde_json::to_value(&original).unwrap();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(
            serde_json::from_value::<RemediationPlan>(changed)
                .unwrap()
                .validate()
                .is_err(),
            "{path}"
        );
    }
}

#[test]
fn target_sets_are_canonical_but_step_order_is_bound() {
    let original = plan();
    let mut reordered = original.clone();
    reordered.targets.reverse();
    reordered.steps[0].targets.reverse();
    reordered.targets.push(target(0));
    reordered.steps[0].targets.push(target(0));
    reordered.recipe.payload =
        serde_json::from_str(r#"{"limit":100,"operation":"selected"}"#).unwrap();
    reordered.seal().unwrap();
    assert_eq!(reordered.fingerprint, original.fingerprint);
    assert_eq!(reordered.targets.len(), 2);
    let mut ordered = original;
    let mut step = ordered.steps[0].clone();
    step.descriptor.id = "second".into();
    step.descriptor.effect = ExpectedEffect::RemovedForCurrentAccountOnly;
    ordered.steps.push(step);
    ordered.seal().unwrap();
    ordered.steps.reverse();
    assert_eq!(ordered.validate(), Err(DomainError::FingerprintMismatch));
}

#[test]
fn sealed_plan_rejects_noncanonical_duplicate_targets() {
    let original = plan();
    let mut duplicated_global_target = original.clone();
    duplicated_global_target.targets.push(target(0));
    assert!(duplicated_global_target.validate().is_err());

    let mut duplicated_step_target = original;
    duplicated_step_target.steps[0].targets.push(target(0));
    assert!(duplicated_step_target.validate().is_err());
}

#[test]
fn sealing_rejects_conflicting_references_missing_targets_and_unsafe_confirmation() {
    let mut conflicting = plan();
    let mut alias = target(0);
    alias.resource.locator_payload["messageId"] = json!("different");
    conflicting.targets.push(alias);
    assert_eq!(conflicting.seal(), Err(DomainError::ConflictingReference));
    let mut missing = plan();
    missing.targets.pop();
    assert!(missing.seal().is_err());
    let mut unsafe_confirmation = plan();
    unsafe_confirmation.confirmation.owner_auth_required = false;
    assert_eq!(
        unsafe_confirmation.seal(),
        Err(DomainError::InvalidConfirmation)
    );
    let mut critical = plan();
    critical.steps[0].descriptor.confirmation_tier = ConfirmationTier::Critical;
    critical.confirmation.tier = ConfirmationTier::Critical;
    assert_eq!(critical.seal(), Err(DomainError::InvalidConfirmation));
}

#[test]
fn broad_cost_and_manual_operations_cannot_automatically_resume() {
    for kind in [
        ActionKind::ClearConversation,
        ActionKind::DeleteByActor,
        ActionKind::DeleteConversation,
    ] {
        let mut broad = plan();
        broad.steps[0].descriptor.kind = kind;
        assert!(broad.seal().is_err());
        broad.restart_policy = RestartPolicy::RequiresNewReview;
        broad.seal().unwrap();
    }
    let mut costly = plan();
    costly.steps[0].descriptor.advisory = Some(ActionAdvisory {
        cost_bearing: true,
        rate_limited: false,
    });
    assert!(costly.seal().is_err());
    let mut unavailable = plan();
    unavailable.steps[0].descriptor.availability = Availability::Unavailable;
    unavailable.steps[0].descriptor.unavailable_reason = Some(SafeError {
        code: ErrorCode::PermissionChanged,
        retry_at: None,
    });
    assert!(unavailable.seal().is_err());
}

#[test]
fn safe_errors_cannot_deserialize_arbitrary_messages_or_details() {
    let safe = SafeError {
        code: ErrorCode::ScopeMismatch,
        retry_at: None,
    };
    assert_eq!(
        safe.message(),
        "This action belongs to a different account or source."
    );
    assert_eq!(
        serde_json::to_value(&safe).unwrap()["message"],
        "This action belongs to a different account or source."
    );
    for field in ["message", "details", "nativeError"] {
        let mut value = serde_json::to_value(&safe).unwrap();
        value[field] = json!("private remote error");
        assert!(serde_json::from_value::<SafeError>(value).is_err());
    }
}

#[test]
fn provider_and_application_errors_are_separate_safe_taxonomies() {
    let provider = [
        "authentication_required",
        "permission_changed",
        "not_found",
        "already_removed",
        "rate_limited",
        "cost_limit_reached",
        "transient",
        "permanent",
        "ambiguous_outcome",
        "unsupported_schema",
        "invalid_archive",
    ];
    for code in provider {
        let retry_at = (code == "rate_limited").then_some("2026-09-03T01:00:00Z");
        let error: ProviderError =
            serde_json::from_value(json!({"code":code,"retryAt":retry_at})).unwrap();
        let encoded = serde_json::to_value(error).unwrap();
        assert_eq!(encoded["code"], code);
        assert!(
            encoded["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
    }
    let application = [
        "unsupported_contract_version",
        "scope_mismatch",
        "stale_context",
        "identity_unavailable",
        "profile_in_use",
        "state_persistence_failed",
        "migration_requires_new_review",
        "restart_requires_new_review",
    ];
    for code in application {
        let error: ApplicationError = serde_json::from_value(json!({"code":code})).unwrap();
        assert_eq!(serde_json::to_value(error).unwrap()["code"], code);
    }
    assert!(
        serde_json::from_value::<ProviderError>(json!({"code":"scope_mismatch","retryAt":null}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<ApplicationError>(json!({"code":"authentication_required"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderError>(
            json!({"code":"permanent","retryAt":null,"nativeError":"private"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderError>(json!({"code":"rate_limited","retryAt":null}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<ProviderError>(
            json!({"code":"permanent","retryAt":"2026-09-03T01:00:00Z"})
        )
        .is_err()
    );
}

#[test]
fn normalized_records_reject_foreign_provenance_and_preserve_metadata() {
    let reference = target(0);
    let record: ContentRecord = serde_json::from_value(json!({
        "id":reference.id, "scope":reference.scope, "resource":reference.resource,
        "conversationId":"29cdec14-7825-5357-aeab-c44c5ee5223b",
        "authorId":"dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        "timestamp":"2026-09-03T00:00:00Z","editedAt":null,"kind":"image",
        "searchableText":"Synthetic searchable text", "attachments":[{
            "kind":"image","safeDisplayName":null,"sizeBytes":123,"mimeType":"image/png",
            "locator":{"schema":"synthetic.attachment","version":1,"payload":{"part":"0007"}}
        }], "replyTo":null,"threadParent":null,"externalLocation":"unavailable",
        "evidence":"live","observedAt":"2026-09-03T00:00:01Z",
        "privacyFindings":["email_address","credential_or_secret"],"detectorVersion":"1",
        "providerMetadata":{"schema":"telegram.content","version":1,
            "payload":{"isOutgoing":true,"isPinned":true,"groupingId":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","originalKind":"photo"}}
    })).unwrap();
    record.validate(&scope()).unwrap();
    let mut foreign = record.clone();
    foreign.scope.source_id =
        serde_json::from_value(json!("22222222-2222-4222-8222-222222222222")).unwrap();
    assert!(foreign.validate(&scope()).is_err());
    let mut no_detector = record.clone();
    no_detector.detector_version = None;
    assert!(no_detector.validate(&scope()).is_err());
    let roundtrip: ContentRecord =
        serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
    assert_eq!(roundtrip, record);
    assert_eq!(
        roundtrip.provider_metadata.unwrap().payload["isPinned"],
        true
    );
    let mut wrong_kind = record;
    wrong_kind.resource.resource_kind = ResourceKind::Conversation;
    assert!(wrong_kind.validate(&scope()).is_err());
}

#[test]
fn source_ownership_is_explicit_and_archive_never_becomes_live() {
    let account: AccountRecord = serde_json::from_value(json!({
        "id":scope().account_id,"provider":"telegram",
        "nativeIdentity":{"schema":"telegram.account","version":1,"payload":{"environment":"test","userId":"42"}},
        "displayName":"Synthetic", "username":null,"avatar":null,"connectionState":"ready",
        "createdAt":"2026-09-03T00:00:00Z","lastSeenAt":"2026-09-03T00:00:01Z"
    })).unwrap();
    let mut source: SourceRecord = serde_json::from_value(json!({
        "id":scope().source_id,"accountId":scope().account_id,"provider":"telegram",
        "kind":"live_connection","state":"ready","archiveFingerprint":null,
        "schemaProfile":{"schema":"telegram.live","version":1,"payload":{}},
        "importedAt":null,"updatedAt":"2026-09-03T00:00:01Z","warnings":[]
    }))
    .unwrap();
    source.validate(&account).unwrap();
    source.account_id =
        serde_json::from_value(json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")).unwrap();
    assert!(source.validate(&account).is_err());
    source.account_id = account.id;
    source.archive_fingerprint = Some("archive-digest".into());
    assert!(source.validate(&account).is_err());
    let mut bad_account = account;
    bad_account.native_identity.version = 0;
    assert!(bad_account.validate().is_err());
}

#[test]
fn conversations_reject_foreign_participants_and_wrong_resource_kind() {
    let reference = fixture()["chats"][0]["ref"].clone();
    let mut conversation: ConversationRecord = serde_json::from_value(json!({
        "id":reference["id"],"scope":scope(),"resource":reference["resource"],
        "kind":"group","title":"Synthetic","parentId":null,"participantCount":null,
        "participants":[],"evidence":"live","observedAt":"2026-09-03T00:00:00Z",
        "providerMetadata":{"schema":"telegram.conversation","version":1,"payload":{"kind":"secret","archived":true}}
    })).unwrap();
    conversation.validate(&scope()).unwrap();
    conversation.resource.resource_kind = ResourceKind::Actor;
    assert!(conversation.validate(&scope()).is_err());
    conversation.resource.resource_kind = ResourceKind::Conversation;
    conversation.provider_metadata.as_mut().unwrap().version = 0;
    assert!(conversation.validate(&scope()).is_err());
    conversation.provider_metadata.as_mut().unwrap().version = 1;
    let mut actor_ref = target(0).resource;
    actor_ref.resource_kind = ResourceKind::Actor;
    actor_ref.locator_schema = "synthetic.actor".into();
    actor_ref.canonical_key = "user:42".into();
    let mut actor: ActorRecord = serde_json::from_value(json!({
        "id":actor_ref.resource_id().unwrap(), "scope":scope(), "resource":actor_ref,
        "displayName":"Synthetic actor", "username":null,"avatar":null,
        "evidence":"live","observedAt":"2026-09-03T00:00:00Z"
    }))
    .unwrap();
    actor.scope.source_id =
        serde_json::from_value(json!("22222222-2222-4222-8222-222222222222")).unwrap();
    conversation.participants.push(actor);
    assert!(conversation.validate(&scope()).is_err());
}

#[test]
fn descriptors_reject_invalid_batches_and_missing_unavailable_reason() {
    let mut descriptor = plan().steps.remove(0).descriptor;
    descriptor.batch.max_targets = 0;
    assert!(descriptor.validate().is_err());
    descriptor.batch.max_targets = 100;
    descriptor.availability = Availability::Unavailable;
    assert!(descriptor.validate().is_err());
    descriptor.unavailable_reason = Some(SafeError {
        code: ErrorCode::PermissionChanged,
        retry_at: None,
    });
    descriptor.validate().unwrap();
    descriptor.availability = Availability::Executable;
    descriptor.unavailable_reason = None;
    descriptor.effect = ExpectedEffect::ContainerDestroyed;
    assert!(descriptor.validate().is_err());
    descriptor.confirmation_tier = ConfirmationTier::Critical;
    descriptor.destructive = false;
    assert!(descriptor.validate().is_err());
    descriptor.destructive = true;
    descriptor.irreversible = false;
    assert!(descriptor.validate().is_err());
    descriptor.irreversible = true;
    descriptor.validate().unwrap();
}

#[test]
fn legacy_history_retains_operation_and_counters_without_executable_scope() {
    let value = json!({
        "id":"eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee","planId":"ffffffff-ffff-4fff-8fff-ffffffffffff",
        "operation":"selected_messages","status":"partial","total":2,"deleted":1,"skipped":3,
        "failed":0,"nextBatch":1,"diagnostics":[{"code":"migration_requires_new_review","retryAt":null}],
        "createdAt":"2026-09-03T00:00:00Z","updatedAt":"2026-09-03T00:00:01Z"
    });
    let history: LegacyHistoryRecord = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(history.total, 2);
    assert_eq!(history.skipped, 3);
    assert_eq!(
        serde_json::to_value(history).unwrap()["operation"],
        "selected_messages"
    );
    for (field, replacement) in [
        ("scope", json!(scope())),
        ("status", json!("queued")),
        ("retryAt", json!("2026-09-03T01:00:00Z")),
        ("recipe", json!({})),
    ] {
        let mut executable = value.clone();
        executable[field] = replacement;
        assert!(serde_json::from_value::<LegacyHistoryRecord>(executable).is_err());
    }
}

#[test]
fn plans_hash_opaque_recipe_fields_and_reject_nil_identity_and_empty_targets() {
    let mut candidate = plan();
    candidate.recipe.payload["fingerprint"] = json!("unscoped");
    candidate.seal().unwrap();
    candidate.validate().unwrap();
    candidate.recipe.payload["fingerprint"] = json!("different-opaque-value");
    assert!(candidate.validate().is_err());
    candidate = plan();
    candidate.id = Uuid::nil();
    assert!(candidate.seal().is_err());
    candidate = plan();
    candidate.steps[0].targets.clear();
    assert!(candidate.seal().is_err());
    candidate = plan();
    candidate.targets.clear();
    assert!(candidate.seal().is_err());
}

#[test]
fn jobs_reject_scope_or_plan_mismatch_and_blocked_retry_but_keep_eligible_counter_semantics() {
    let plan = plan();
    let mut job: ScopedJobRecord = serde_json::from_value(json!({
        "id":"eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee", "planId":plan.id,
        "scope":scope(),"dirtyRefs":[target(0)],"status":"running",
        "counters":{"selected":5,"eligible":2,"deleted":1,"skipped":3,"failed":0,"uncertain":0},
        "nextBatch":1,"retryAt":null,"diagnostics":[],"startedAuthorized":true,
        "createdAt":"2026-09-03T00:00:00Z","updatedAt":"2026-09-03T00:00:01Z"
    }))
    .unwrap();
    job.validate(&plan).unwrap();
    job.scope.source_id =
        serde_json::from_value(json!("22222222-2222-4222-8222-222222222222")).unwrap();
    assert!(job.validate(&plan).is_err());
    job.scope = scope();
    job.plan_id = Uuid::nil();
    assert!(job.validate(&plan).is_err());
    job.plan_id = plan.id;
    job.status = JobStatus::Blocked;
    assert!(job.validate(&plan).is_err());
    job.diagnostics.push(SafeError {
        code: ErrorCode::IdentityUnavailable,
        retry_at: None,
    });
    job.validate(&plan).unwrap();
    job.retry_at = Some(job.updated_at);
    assert!(job.validate(&plan).is_err());
    job.retry_at = None;
    job.status = JobStatus::Partial;
    job.counters.deleted = 3;
    assert!(job.validate(&plan).is_err());
    job.counters.deleted = 1;
    job.counters.uncertain = 1;
    job.status = JobStatus::Completed;
    assert!(job.validate(&plan).is_err());
}
