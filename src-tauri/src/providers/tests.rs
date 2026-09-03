use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use retract_domain::{
    AccountId, AccountRecord, ActionDescriptor, ActionKind, ActionStep, ActorId, Availability,
    BatchConstraints, ConfirmationRequirements, ConfirmationTier, ConnectionState, ContentId,
    ContentKind, ContentRecord, ConversationId, ConversationRecord, EvidenceState, ExpectedEffect,
    ExternalLocationAvailability, ProviderKey, ProviderResourceRef, RemediationPlan, ResourceKind,
    RestartPolicy, Scope, SourceId, SourceKind, SourceRecord, SourceState, VersionedPayload,
};
use serde_json::json;
use uuid::Uuid;

use super::{
    ports::{
        ContentQuery, ConversationQuery, Page, ProviderCapability, ProviderDescriptor,
        ProviderRegistration, QuerySource, ResolveRequest,
    },
    registry::{ProviderRegistry, ProviderRegistryError},
    telegram::identity::{SessionBinding, TelegramAccountProfile, VerifiedTelegramIdentity},
    telegram::locators::{
        TelegramAccountLocator, TelegramActorKind, TelegramActorLocator,
        TelegramConversationLocator, TelegramEnvironment, TelegramGroupingLocator,
        TelegramMessageLocator, TelegramPayloadValidator, TelegramRemediationRecipe,
        TelegramSourceProfile, telegram_provider_key,
    },
};

fn provider_key(value: &str) -> ProviderKey {
    ProviderKey::try_from(value.to_owned()).unwrap()
}

fn scope(provider: &str) -> Scope {
    Scope {
        provider: provider_key(provider),
        account_id: AccountId::try_from(Uuid::from_u128(0x100)).unwrap(),
        source_id: SourceId::try_from(Uuid::from_u128(0x200)).unwrap(),
    }
}

#[derive(Debug)]
struct NonnumericQuery;

#[async_trait]
impl QuerySource for NonnumericQuery {
    async fn list_conversations(
        &self,
        _request: ConversationQuery,
    ) -> Result<Page<ConversationRecord>, retract_domain::ProviderError> {
        Ok(Page {
            items: Vec::new(),
            next_cursor: Some("conversation:page/0002".into()),
        })
    }

    async fn search(
        &self,
        request: ContentQuery,
    ) -> Result<Page<ContentRecord>, retract_domain::ProviderError> {
        assert_eq!(request.query, "opaque");
        let resource = ProviderResourceRef {
            provider: request.scope.provider.clone(),
            account_id: request.scope.account_id,
            resource_kind: ResourceKind::Content,
            locator_schema: "opaque.message".into(),
            locator_version: 1,
            canonical_key: "message:part/0007".into(),
            locator_payload: json!({ "key": "message:part/0007" }),
        };
        let item = ContentRecord {
            id: ContentId::try_from(resource.resource_id().unwrap()).unwrap(),
            scope: request.scope,
            conversation_id: ConversationId::try_from(Uuid::from_u128(0x10)).unwrap(),
            resource,
            author_id: ActorId::try_from(Uuid::from_u128(0x20)).unwrap(),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
            edited_at: None,
            kind: ContentKind::Text,
            searchable_text: "Synthetic opaque content".into(),
            attachments: Vec::new(),
            reply_to: None,
            thread_parent: None,
            external_location: ExternalLocationAvailability::Unsupported,
            evidence: EvidenceState::Live,
            observed_at: Utc.timestamp_opt(1_700_000_001, 0).single().unwrap(),
            privacy_findings: Vec::new(),
            detector_version: None,
            provider_metadata: None,
        };
        Ok(Page {
            items: vec![item],
            next_cursor: Some("message:part/0007".into()),
        })
    }

    async fn resolve(
        &self,
        request: ResolveRequest,
    ) -> Result<Vec<ContentRecord>, retract_domain::ProviderError> {
        assert!(request.refs.is_empty());
        Ok(Vec::new())
    }
}

struct QueryOnlyRegistration {
    descriptor: ProviderDescriptor,
    query: Option<Arc<dyn QuerySource>>,
}

impl ProviderRegistration for QueryOnlyRegistration {
    fn descriptor(&self) -> ProviderDescriptor {
        self.descriptor.clone()
    }

    fn query_source(&self) -> Result<Arc<dyn QuerySource>, ProviderRegistryError> {
        self.query
            .clone()
            .ok_or_else(|| ProviderRegistryError::unsupported(ProviderCapability::ContentSearch))
    }
}

fn query_registration(key: &str) -> Arc<dyn ProviderRegistration> {
    Arc::new(QueryOnlyRegistration {
        descriptor: ProviderDescriptor {
            key: provider_key(key),
            display_name: "Synthetic opaque query".into(),
            capabilities: BTreeSet::from([
                ProviderCapability::ConversationListing,
                ProviderCapability::ContentSearch,
            ]),
        },
        query: Some(Arc::new(NonnumericQuery)),
    })
}

#[test]
fn registry_routes_a_nonnumeric_query_provider_without_parsing_its_cursor() {
    tauri::async_runtime::block_on(async {
        let registry = ProviderRegistry::default();
        registry
            .register(query_registration("opaque_test"))
            .unwrap();

        let registration = registry.get(&provider_key("opaque_test")).unwrap();
        let query = registration.query_source().unwrap();
        let page = query
            .search(ContentQuery {
                scope: scope("opaque_test"),
                query: "opaque".into(),
                cursor: Some("message:part/0006".into()),
                limit: 25,
            })
            .await
            .unwrap();

        assert_eq!(page.next_cursor.as_deref(), Some("message:part/0007"));
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].resource.canonical_key, "message:part/0007");
        assert_eq!(
            page.items[0].resource.locator_payload["key"],
            "message:part/0007"
        );
        page.items[0].validate(&scope("opaque_test")).unwrap();
    });
}

#[test]
fn registry_rejects_duplicate_unknown_and_declared_capabilities_without_ports() {
    let registry = ProviderRegistry::default();
    registry
        .register(query_registration("opaque_test"))
        .unwrap();

    assert!(matches!(
        registry.register(query_registration("opaque_test")),
        Err(ProviderRegistryError::DuplicateProvider(key)) if key == provider_key("opaque_test")
    ));
    assert!(matches!(
        registry.get(&provider_key("missing")),
        Err(ProviderRegistryError::UnknownProvider(key)) if key == provider_key("missing")
    ));

    for capability in [
        ProviderCapability::MediaMetadata,
        ProviderCapability::ExternalLocation,
    ] {
        let missing = Arc::new(QueryOnlyRegistration {
            descriptor: ProviderDescriptor {
                key: provider_key("missing_metadata_port"),
                display_name: "Missing query port".into(),
                capabilities: BTreeSet::from([capability]),
            },
            query: None,
        });
        assert!(matches!(
            registry.register(missing),
            Err(ProviderRegistryError::MissingPort { .. })
        ));
    }

    let missing = Arc::new(QueryOnlyRegistration {
        descriptor: ProviderDescriptor {
            key: provider_key("missing_port"),
            display_name: "Missing query port".into(),
            capabilities: BTreeSet::from([ProviderCapability::ContentSearch]),
        },
        query: None,
    });
    assert!(matches!(
        registry.register(missing),
        Err(ProviderRegistryError::MissingPort {
            capability: ProviderCapability::ContentSearch,
            ..
        })
    ));
}

#[test]
fn unsupported_ports_return_an_explicit_error_instead_of_a_success_stub() {
    let registration = query_registration("opaque_test");
    assert!(matches!(
        registration.remediation(),
        Err(ProviderRegistryError::UnsupportedCapability(
            ProviderCapability::AutomaticRemediation
        ))
    ));
    assert!(matches!(
        registration.live_connection(),
        Err(ProviderRegistryError::UnsupportedCapability(
            ProviderCapability::LiveConnection
        ))
    ));
    assert!(matches!(
        registration.import_inspector(),
        Err(ProviderRegistryError::UnsupportedCapability(
            ProviderCapability::ImportInspection
        ))
    ));
}

#[test]
fn provider_request_and_result_contracts_are_strict_and_string_cursor_based() {
    let request = serde_json::from_value::<ConversationQuery>(json!({
        "scope": scope("opaque_test"),
        "cursor": "conversation:page/0002",
        "limit": 50
    }))
    .unwrap();
    assert_eq!(request.cursor.as_deref(), Some("conversation:page/0002"));
    assert!(
        serde_json::from_value::<ConversationQuery>(json!({
            "scope": scope("opaque_test"),
            "cursor": "conversation:page/0002",
            "limit": 50,
            "nativeChatId": 7
        }))
        .is_err()
    );

    let state = ConnectionState::Ready;
    assert_eq!(serde_json::to_value(state).unwrap(), json!("ready"));
    assert_eq!(
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .unwrap()
            .timestamp(),
        1_700_000_000
    );
}

fn account_id(value: u128) -> AccountId {
    AccountId::try_from(Uuid::from_u128(value)).unwrap()
}

fn source_id(value: u128) -> SourceId {
    SourceId::try_from(Uuid::from_u128(value)).unwrap()
}

fn telegram_account(
    id: AccountId,
    environment: TelegramEnvironment,
    user_id: &str,
) -> AccountRecord {
    AccountRecord {
        id,
        provider: telegram_provider_key(),
        native_identity: TelegramAccountLocator::new(environment, user_id)
            .unwrap()
            .into_payload(),
        display_name: "Synthetic account".into(),
        username: Some("synthetic".into()),
        avatar: None,
        connection_state: ConnectionState::Ready,
        created_at: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
        last_seen_at: Utc.timestamp_opt(1_700_000_001, 0).single().unwrap(),
    }
}

fn telegram_source(account: &AccountRecord, id: SourceId) -> SourceRecord {
    SourceRecord {
        id,
        account_id: account.id,
        provider: telegram_provider_key(),
        kind: SourceKind::LiveConnection,
        state: SourceState::Ready,
        archive_fingerprint: None,
        schema_profile: TelegramSourceProfile::payload(),
        imported_at: None,
        updated_at: Utc.timestamp_opt(1_700_000_001, 0).single().unwrap(),
        warnings: Vec::new(),
    }
}

#[test]
fn telegram_decimal_locators_enforce_native_ranges_and_canonical_spelling() {
    for valid in ["-9223372036854775808", "-1", "1", "9223372036854775807"] {
        assert!(TelegramConversationLocator::new(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "",
        "0",
        "-0",
        " 1",
        "1 ",
        "+1",
        "01",
        "-01",
        "1.0",
        "1e3",
        "9223372036854775808",
    ] {
        assert!(
            TelegramConversationLocator::new(invalid).is_err(),
            "{invalid}"
        );
    }
    for valid in [
        "1",
        "9007199254740992",
        "9007199254740993",
        "9223372036854775807",
    ] {
        assert!(
            TelegramMessageLocator::new("-100", valid).is_ok(),
            "{valid}"
        );
        assert!(TelegramAccountLocator::new(TelegramEnvironment::Test, valid).is_ok());
    }
    for invalid in ["0", "-1", "+1", "01", "1.0", "1e3"] {
        assert!(
            TelegramMessageLocator::new("-100", invalid).is_err(),
            "{invalid}"
        );
        assert!(TelegramAccountLocator::new(TelegramEnvironment::Test, invalid).is_err());
    }
}

#[test]
fn telegram_grouping_ids_preserve_nonzero_signed_tdlib_album_ids() {
    use crate::persistence::ProviderPayloadValidator;
    let validator = TelegramPayloadValidator;
    let account = account_id(0x111);
    for valid in ["-1", "-9223372036854775808", "9223372036854775807"] {
        let first = TelegramGroupingLocator::new("-100", valid)
            .unwrap()
            .resource(account);
        let second = TelegramGroupingLocator::new("-101", valid)
            .unwrap()
            .resource(account);
        validator.validate_resource(&first).unwrap();
        validator.validate_resource(&second).unwrap();
        assert_ne!(first.canonical_key, second.canonical_key);
        assert_ne!(first.resource_id().unwrap(), second.resource_id().unwrap());
    }
    for invalid in [
        "0",
        "-0",
        " 1",
        "1 ",
        "+1",
        "01",
        "-01",
        "1.0",
        "1e3",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        assert!(
            TelegramGroupingLocator::new("-100", invalid).is_err(),
            "{invalid}"
        );
        let mut resource = TelegramGroupingLocator::new("-100", "77")
            .unwrap()
            .resource(account);
        resource.locator_payload["groupingId"] = json!(invalid);
        resource.canonical_key =
            serde_json::to_string(&["telegram-grouping-v1", "-100", invalid]).unwrap();
        assert!(validator.validate_resource(&resource).is_err(), "{invalid}");
    }
}

#[test]
fn telegram_message_actor_and_grouping_keys_are_unambiguous_and_chat_scoped() {
    let account = account_id(0x111);
    let first = TelegramMessageLocator::new("-100", "9007199254740993")
        .unwrap()
        .resource(account);
    let second = TelegramMessageLocator::new("-101", "9007199254740993")
        .unwrap()
        .resource(account);
    assert_ne!(first.canonical_key, second.canonical_key);
    assert_ne!(first.resource_id().unwrap(), second.resource_id().unwrap());

    let user = TelegramActorLocator::new(TelegramActorKind::User, "42").unwrap();
    let chat = TelegramActorLocator::new(TelegramActorKind::Chat, "42").unwrap();
    assert_ne!(user.canonical_key(), chat.canonical_key());

    let first_group = TelegramGroupingLocator::new("-100", "77").unwrap();
    let second_group = TelegramGroupingLocator::new("-101", "77").unwrap();
    assert_ne!(first_group.canonical_key(), second_group.canonical_key());
    assert_ne!(
        first_group.resource(account).resource_id().unwrap(),
        second_group.resource(account).resource_id().unwrap()
    );
}

#[test]
fn telegram_locator_payloads_deny_unknown_fields_and_have_a_frozen_application_id() {
    assert!(
        serde_json::from_value::<TelegramMessageLocator>(json!({
            "chatId": "-100",
            "messageId": "7",
            "preview": "must never be accepted"
        }))
        .is_err()
    );

    let resource = TelegramMessageLocator::new("-100", "9007199254740993")
        .unwrap()
        .resource(account_id(0x111));
    assert_eq!(
        resource.resource_id().unwrap(),
        Uuid::parse_str("3148b55f-2dff-5584-b49e-8e991f31cdef").unwrap()
    );
}

#[test]
fn telegram_payload_validator_rejects_wrong_schema_version_key_fields_and_ranges() {
    let validator = TelegramPayloadValidator;
    let account = telegram_account(account_id(0x111), TelegramEnvironment::Production, "42");
    let source = telegram_source(&account, source_id(0x222));
    let resource = TelegramConversationLocator::new("-100")
        .unwrap()
        .resource(account.id);

    crate::persistence::ProviderPayloadValidator::validate_account(&validator, &account).unwrap();
    crate::persistence::ProviderPayloadValidator::validate_source(&validator, &source, &account)
        .unwrap();
    crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &resource).unwrap();

    let mut wrong_schema = resource.clone();
    wrong_schema.locator_schema = "telegram.unknown".into();
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &wrong_schema)
            .is_err()
    );
    let mut wrong_version = resource.clone();
    wrong_version.locator_version = 2;
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &wrong_version)
            .is_err()
    );
    let mut wrong_key = resource.clone();
    wrong_key.canonical_key = "[\"telegram-conversation-v1\",\"-101\"]".into();
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &wrong_key)
            .is_err()
    );
    let mut wrong_fields = resource.clone();
    wrong_fields.locator_payload["title"] = json!("private content");
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &wrong_fields)
            .is_err()
    );
    let mut wrong_range = resource;
    wrong_range.locator_payload["chatId"] = json!("0");
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_resource(&validator, &wrong_range)
            .is_err()
    );
}

fn telegram_plan() -> RemediationPlan {
    let account_id = account_id(0x111);
    let scope = Scope {
        provider: telegram_provider_key(),
        account_id,
        source_id: source_id(0x222),
    };
    let resource = TelegramMessageLocator::new("-100", "7")
        .unwrap()
        .scoped(scope.clone());
    let descriptor = ActionDescriptor {
        id: "telegram.delete_for_everyone".into(),
        kind: ActionKind::DeleteRemoteItem,
        effect: ExpectedEffect::RemovedForAllParticipants,
        availability: Availability::Executable,
        unavailable_reason: None,
        requires_live_preflight: false,
        batch: BatchConstraints {
            max_targets: 100,
            max_parallel: 1,
        },
        confirmation_tier: ConfirmationTier::High,
        destructive: true,
        irreversible: true,
        advisory: None,
    };
    let steps = vec![ActionStep {
        descriptor,
        targets: vec![resource.clone()],
    }];
    let recipe = TelegramRemediationRecipe::for_steps(&steps).unwrap();
    let mut plan = RemediationPlan {
        id: Uuid::from_u128(0x333),
        scope,
        steps,
        targets: vec![resource],
        confirmation: ConfirmationRequirements {
            tier: ConfirmationTier::High,
            acknowledgement_required: true,
            owner_auth_required: true,
            exact_text: Some("Synthetic account".into()),
        },
        recipe,
        restart_policy: RestartPolicy::ResumeFrozenTargets,
        created_at: Utc.timestamp_opt(1_700_000_002, 0).single().unwrap(),
        fingerprint: String::new(),
    };
    plan.seal().unwrap();
    plan
}

#[test]
fn telegram_recipe_validation_is_typed_content_free_and_requires_exact_target_agreement() {
    let validator = TelegramPayloadValidator;
    let plan = telegram_plan();
    crate::persistence::ProviderPayloadValidator::validate_recipe(&validator, &plan).unwrap();
    assert!(
        !serde_json::to_string(&plan.recipe)
            .unwrap()
            .contains("Synthetic account")
    );

    let mut wrong_target = plan.clone();
    wrong_target.recipe.payload["steps"][0]["targets"][0]["canonicalKey"] =
        json!("[\"telegram-message-v1\",\"-100\",\"8\"]");
    wrong_target.seal().unwrap();
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_recipe(&validator, &wrong_target)
            .is_err()
    );

    let mut unknown = plan;
    unknown.recipe = VersionedPayload {
        schema: "telegram.remediation_recipe".into(),
        version: 2,
        payload: json!({ "steps": [] }),
    };
    unknown.seal().unwrap();
    assert!(
        crate::persistence::ProviderPayloadValidator::validate_recipe(&validator, &unknown)
            .is_err()
    );
}

fn open_telegram_store(
    profile: &std::path::Path,
    label: &str,
) -> Arc<crate::persistence::FoundationStore> {
    crate::persistence::FoundationStore::open_with_test_key_and_payload_validator(
        profile.to_path_buf(),
        crate::persistence::StoreBinding {
            provider: telegram_provider_key(),
            profile: label.into(),
        },
        [0x44; 32],
        Arc::new(TelegramPayloadValidator),
    )
    .unwrap()
}

fn verified_identity(
    environment: TelegramEnvironment,
    user_id: i64,
    generation: u128,
) -> VerifiedTelegramIdentity {
    VerifiedTelegramIdentity::new(environment, user_id, Uuid::from_u128(generation)).unwrap()
}

fn account_profile(display_name: &str) -> TelegramAccountProfile {
    TelegramAccountProfile {
        display_name: display_name.into(),
        username: Some("synthetic".into()),
    }
}

#[test]
fn session_binding_persists_before_publish_and_reuses_ids_after_restart_and_rename() {
    let directory = tempfile::tempdir().unwrap();
    let profile_path = directory.path().join("profile");
    let first_store = open_telegram_store(&profile_path, "stable-profile-label");
    let first_binding = SessionBinding::default();
    let first_identity = verified_identity(TelegramEnvironment::Production, 42, 0x401);
    first_binding
        .begin_generation(first_identity.session_generation)
        .unwrap();
    let first = first_binding
        .publish(
            &first_store,
            &first_identity,
            &account_profile("Original display name"),
        )
        .unwrap();
    assert_eq!(first_binding.current(), Some(first.clone()));
    drop(first_store);

    let restarted_store = open_telegram_store(&profile_path, "stable-profile-label");
    let restarted_binding = SessionBinding::default();
    let restarted_identity = verified_identity(TelegramEnvironment::Production, 42, 0x402);
    restarted_binding
        .begin_generation(restarted_identity.session_generation)
        .unwrap();
    let restarted = restarted_binding
        .publish(
            &restarted_store,
            &restarted_identity,
            &account_profile("Renamed display name"),
        )
        .unwrap();

    assert_eq!(first.scope, restarted.scope);
    assert_ne!(first.session_generation, restarted.session_generation);
    let snapshot = restarted_store.snapshot().unwrap();
    assert_eq!(snapshot.identities.len(), 1);
    assert_eq!(snapshot.sources.len(), 1);
    assert_eq!(snapshot.identities[0].display_name, "Renamed display name");
}

#[test]
fn session_binding_separates_accounts_and_environments_and_invalidates_stale_contexts() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_telegram_store(&directory.path().join("profile"), "multi-account-profile");
    let binding = SessionBinding::default();

    let production = verified_identity(TelegramEnvironment::Production, 42, 0x501);
    binding
        .begin_generation(production.session_generation)
        .unwrap();
    let production_context = binding
        .publish(&store, &production, &account_profile("Production"))
        .unwrap();
    binding.invalidate();
    assert!(binding.validate(&production_context).is_err());

    let test = verified_identity(TelegramEnvironment::Test, 42, 0x502);
    binding.begin_generation(test.session_generation).unwrap();
    let test_context = binding
        .publish(&store, &test, &account_profile("Test"))
        .unwrap();
    assert_ne!(production_context.scope, test_context.scope);

    binding.invalidate();
    let other = verified_identity(TelegramEnvironment::Production, 43, 0x503);
    binding.begin_generation(other.session_generation).unwrap();
    let other_context = binding
        .publish(&store, &other, &account_profile("Other"))
        .unwrap();
    assert_ne!(production_context.scope, other_context.scope);
    assert_ne!(test_context.scope, other_context.scope);
}

#[test]
fn failed_foundation_transaction_leaves_session_context_absent() {
    let directory = tempfile::tempdir().unwrap();
    let profile_path = directory.path().join("profile");
    let store = open_telegram_store(&profile_path, "failing-profile");
    std::fs::create_dir(profile_path.join("jobs.enc.tmp")).unwrap();
    let binding = SessionBinding::default();
    let identity = verified_identity(TelegramEnvironment::Production, 42, 0x601);
    binding
        .begin_generation(identity.session_generation)
        .unwrap();

    assert!(matches!(
        binding.publish(&store, &identity, &account_profile("No publication")),
        Err(crate::error::AppError::StatePersistenceFailed)
    ));
    assert_eq!(binding.current(), None);
    assert!(store.snapshot().unwrap().identities.is_empty());
}

#[test]
fn telegram_validator_rejects_untyped_account_and_nonlive_source_payloads() {
    use crate::persistence::ProviderPayloadValidator;
    let validator = TelegramPayloadValidator;
    let mut account = telegram_account(account_id(0x111), TelegramEnvironment::Production, "42");
    account.avatar = Some(VersionedPayload {
        schema: "untyped.avatar".into(),
        version: 1,
        payload: json!({ "privateFileName": "sensitive" }),
    });
    assert!(validator.validate_account(&account).is_err());

    account.avatar = None;
    let mut source = telegram_source(&account, source_id(0x222));
    source.archive_fingerprint = Some("not-a-live-source".into());
    assert!(validator.validate_source(&source, &account).is_err());
}

#[test]
fn concrete_telegram_validator_guards_foundation_transactions_and_native_identity_uniqueness() {
    use crate::persistence::ProviderPayloadValidator;
    let directory = tempfile::tempdir().unwrap();
    let store = open_telegram_store(&directory.path().join("profile"), "typed-payloads");
    let account = telegram_account(account_id(0x111), TelegramEnvironment::Production, "42");
    let source = telegram_source(&account, source_id(0x222));
    let plan = telegram_plan();
    store
        .transaction(|state| {
            state.identities.push(account.clone());
            state.sources.push(source);
            state.plans.push(plan);
            Ok(())
        })
        .unwrap();

    assert!(
        store
            .transaction(|state| {
                state.plans[0].recipe.payload["messageBody"] = json!("must not persist");
                state.plans[0].seal().unwrap();
                Ok(())
            })
            .is_err()
    );
    assert!(
        store
            .transaction(|state| {
                let mut alias = account.clone();
                alias.id = account_id(0x999);
                alias.display_name = "Renamed alias".into();
                state.identities.push(alias);
                Ok(())
            })
            .is_err()
    );

    let first = TelegramPayloadValidator;
    let equivalent = TelegramPayloadValidator;
    assert_eq!(
        first.validation_policy_key(),
        equivalent.validation_policy_key()
    );
    let native = first.validate_account(&account).unwrap();
    let mut renamed = account.clone();
    renamed.display_name = "Renamed".into();
    assert_eq!(native, first.validate_account(&renamed).unwrap());
    let test_account = telegram_account(account.id, TelegramEnvironment::Test, "42");
    assert_ne!(native, first.validate_account(&test_account).unwrap());
}

#[test]
fn remediation_port_contracts_keep_fingerprints_targets_and_outcomes_explicit() {
    use super::ports::{
        BatchItemResult, BatchOutcome, BatchResult, ExecutionBatch, PreflightRequest,
        PreflightResult, VerificationItem, VerificationRequest, VerificationResult,
        VerificationState,
    };
    let plan = telegram_plan();
    let context = retract_domain::ActiveContext {
        scope: plan.scope.clone(),
        session_generation: Uuid::from_u128(0x777),
    };
    let preflight = PreflightRequest {
        context: context.clone(),
        plan: plan.clone(),
    };
    let result = PreflightResult {
        plan_id: plan.id,
        fingerprint: plan.fingerprint.clone(),
        descriptors: vec![plan.steps[0].descriptor.clone()],
    };
    assert_eq!(
        serde_json::to_value(&preflight).unwrap()["plan"]["fingerprint"],
        plan.fingerprint
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap()["descriptors"][0]["effect"],
        "removed_for_all_participants"
    );
    let batch = ExecutionBatch {
        context: context.clone(),
        plan_id: plan.id,
        fingerprint: plan.fingerprint.clone(),
        step_index: 0,
        action: ActionKind::DeleteRemoteItem,
        targets: plan.targets.clone(),
        recipe: plan.recipe.clone(),
    };
    let batch_result = BatchResult {
        plan_id: plan.id,
        fingerprint: plan.fingerprint.clone(),
        items: vec![BatchItemResult {
            target: plan.targets[0].clone(),
            outcome: BatchOutcome::Uncertain,
            diagnostic: None,
        }],
    };
    let verification = VerificationRequest {
        context,
        plan_id: plan.id,
        fingerprint: plan.fingerprint.clone(),
        action: ActionKind::DeleteRemoteItem,
        targets: plan.targets.clone(),
        recipe: plan.recipe,
    };
    let verified = VerificationResult {
        plan_id: plan.id,
        fingerprint: plan.fingerprint,
        items: vec![VerificationItem {
            target: plan.targets[0].clone(),
            state: VerificationState::Uncertain,
            diagnostic: None,
        }],
    };
    assert_eq!(
        serde_json::to_value(batch).unwrap()["targets"][0]["resource"]["locatorPayload"]["messageId"],
        "7"
    );
    assert_eq!(
        serde_json::to_value(batch_result).unwrap()["items"][0]["outcome"],
        "uncertain"
    );
    assert_eq!(
        serde_json::to_value(verification).unwrap()["action"],
        "delete_remote_item"
    );
    assert_eq!(
        serde_json::to_value(verified).unwrap()["items"][0]["state"],
        "uncertain"
    );
}

#[test]
fn setup_and_default_synthetic_gateways_do_not_invent_verified_identity() {
    use crate::gateway::TelegramGateway;
    let setup = crate::setup_gateway::SetupGateway::new("Synthetic setup");
    assert_eq!(setup.verified_identity(), None);
    assert_eq!(
        crate::demo_gateway::DemoGateway::new().verified_identity(),
        None
    );
    let identity = verified_identity(TelegramEnvironment::Test, 42, 0x888);
    let explicit = crate::demo_gateway::DemoGateway::with_verified_identity(identity.clone());
    assert_eq!(explicit.verified_identity(), Some(identity));
}
