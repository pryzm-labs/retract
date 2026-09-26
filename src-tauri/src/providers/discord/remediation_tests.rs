use super::remediation::{DiscordMessageDelete, DiscordRemediationIo, authorization_reason};
use super::session::{
    DiscordCredentialStore, DiscordIdentityClient, DiscordSessionOwner, StoredDiscordCredential,
    VerifiedDiscordIdentity,
};
use super::{
    DiscordNormalizer,
    http::{DeleteOutcome, DiscordDeleteError, DiscordLiveMessageBinding},
};
use async_trait::async_trait;
use discord_archive::{ChannelContext, DiscordId, ExportAccount, SentMessage};
use retract_domain::*;
use std::sync::{Arc, Mutex, atomic::AtomicBool};

const OWNER: &str = "9007199254741001";
const CHANNEL: &str = "9007199254741101";
const MESSAGE: &str = "1985931830091579393";
const TOKEN: &str = "synthetic.discord.token-value_123456789";

struct Store;
impl DiscordCredentialStore for Store {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, crate::error::AppError> {
        Ok(None)
    }
    fn save(&self, _: &StoredDiscordCredential) -> Result<(), crate::error::AppError> {
        Ok(())
    }
    fn forget(&self) -> Result<(), crate::error::AppError> {
        Ok(())
    }
}

struct Identity;
#[async_trait]
impl DiscordIdentityClient for Identity {
    async fn verify(&self, _: &str) -> Result<VerifiedDiscordIdentity, crate::error::AppError> {
        Ok(VerifiedDiscordIdentity {
            account_id: OWNER.into(),
            username: "owner".into(),
            display_name: "Owner".into(),
        })
    }
}

struct Query(Vec<ContentRecord>);
#[async_trait]
impl crate::providers::ports::QuerySource for Query {
    async fn list_conversations(
        &self,
        _: crate::providers::ports::ConversationQuery,
    ) -> Result<crate::providers::ports::Page<ConversationRecord>, ProviderError> {
        unreachable!()
    }
    async fn search(
        &self,
        _: crate::providers::ports::ContentQuery,
    ) -> Result<crate::providers::ports::Page<ContentRecord>, ProviderError> {
        unreachable!()
    }
    async fn resolve(
        &self,
        request: crate::providers::ports::ResolveRequest,
    ) -> Result<Vec<ContentRecord>, ProviderError> {
        Ok(self
            .0
            .iter()
            .filter(|record| {
                request
                    .refs
                    .iter()
                    .any(|target| target.id == *record.id.as_uuid())
            })
            .cloned()
            .collect())
    }
}

#[derive(Default)]
struct Delete {
    probes: Mutex<Vec<(String, String, String, String)>>,
    deletes: Mutex<Vec<(String, String, String, String)>>,
    probe_error: Mutex<Option<DiscordDeleteError>>,
    expected_binding: Option<DiscordLiveMessageBinding>,
}

impl Delete {
    fn rejecting(error: DiscordDeleteError) -> Self {
        Self {
            probe_error: Mutex::new(Some(error)),
            ..Self::default()
        }
    }

    fn requiring(expected_binding: DiscordLiveMessageBinding) -> Self {
        Self {
            expected_binding: Some(expected_binding),
            ..Self::default()
        }
    }

    fn check_binding(&self, binding: &DiscordLiveMessageBinding) -> Result<(), DiscordDeleteError> {
        if self
            .expected_binding
            .as_ref()
            .is_some_and(|expected| expected != binding)
        {
            Err(DiscordDeleteError::OwnershipMismatch)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl DiscordMessageDelete for Delete {
    async fn verify_owned(
        &self,
        channel: &str,
        message: &str,
        owner: &str,
        binding: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(DiscordDeleteError::Cancelled);
        }
        self.check_binding(binding)?;
        self.probes.lock().unwrap().push((
            channel.into(),
            message.into(),
            owner.into(),
            token.into(),
        ));
        if let Some(error) = *self.probe_error.lock().unwrap() {
            return Err(error);
        }
        Ok(())
    }

    async fn delete_owned(
        &self,
        channel: &str,
        message: &str,
        owner: &str,
        binding: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<DeleteOutcome, super::http::DiscordDeleteError> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(DiscordDeleteError::Cancelled);
        }
        self.check_binding(binding)?;
        self.deletes.lock().unwrap().push((
            channel.into(),
            message.into(),
            owner.into(),
            token.into(),
        ));
        Ok(DeleteOutcome::Deleted)
    }
}

fn fixture() -> (ActiveContext, ContentRecord) {
    let scope = Scope {
        provider: "discord".to_owned().try_into().unwrap(),
        account_id: uuid::Uuid::from_u128(10).try_into().unwrap(),
        source_id: uuid::Uuid::from_u128(11).try_into().unwrap(),
    };
    let context = ActiveContext {
        scope: scope.clone(),
        session_generation: uuid::Uuid::from_u128(12),
    };
    let account = ExportAccount {
        id: DiscordId::parse(OWNER).unwrap(),
        username: "owner".into(),
    };
    let channel = ChannelContext {
        id: DiscordId::parse(CHANNEL).unwrap(),
        source_type: "direct".into(),
        name: Some("Archive DM".into()),
        recipients: Some(vec!["recipient".into()]),
        guild: None,
    };
    let message = SentMessage {
        id: DiscordId::parse(MESSAGE).unwrap(),
        account_id: account.id.clone(),
        channel_id: channel.id.clone(),
        timestamp_millis: 1_893_553_445_123,
        contents: "synthetic message".into(),
        attachments: String::new(),
    };
    let record = DiscordNormalizer::new(scope, "2031-01-02T03:04:05Z".parse().unwrap())
        .unwrap()
        .content(&account, &channel, &message)
        .unwrap();
    (context, record)
}

fn record_for(scope: Scope, channel_id: &str, message_id: &str) -> ContentRecord {
    let account = ExportAccount {
        id: DiscordId::parse(OWNER).unwrap(),
        username: "owner".into(),
    };
    let channel = ChannelContext {
        id: DiscordId::parse(channel_id).unwrap(),
        source_type: "direct".into(),
        name: Some("Archive DM".into()),
        recipients: Some(vec!["recipient".into()]),
        guild: None,
    };
    let message = SentMessage {
        id: DiscordId::parse(message_id).unwrap(),
        account_id: account.id.clone(),
        channel_id: channel.id.clone(),
        timestamp_millis: 1_893_553_445_123,
        contents: "private content must not enter the native prompt".into(),
        attachments: String::new(),
    };
    DiscordNormalizer::new(scope, "2031-01-02T03:04:05Z".parse().unwrap())
        .unwrap()
        .content(&account, &channel, &message)
        .unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn ready_session() -> Arc<DiscordSessionOwner> {
    let session = Arc::new(DiscordSessionOwner::with_dependencies(
        Arc::new(Identity),
        Arc::new(Store),
    ));
    runtime()
        .block_on(session.submit_manual(OWNER, TOKEN, false))
        .unwrap();
    session
}

fn content_ref(record: &ContentRecord) -> ScopedResourceRef {
    ScopedResourceRef {
        scope: record.scope.clone(),
        id: *record.id.as_uuid(),
        resource: record.resource.clone(),
    }
}

#[test]
fn reviewed_plan_contains_only_exact_owner_archive_targets_and_risk_confirmation() {
    let (context, record) = fixture();
    let target = content_ref(&record);
    let delete = Arc::new(Delete::default());
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![record])),
        ready_session(),
        delete.clone(),
    )
    .unwrap();
    let rt = runtime();
    let intents = rt
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::intents(
                &io,
                &context,
                vec![target.clone()],
            ),
        )
        .unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].action_id, "selected_messages");
    assert_eq!(intents[0].descriptors[0].batch.max_targets, 1);
    let plan = rt
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                &io,
                &context,
                crate::providers::ports::PrepareIntent {
                    action_id: "selected_messages".into(),
                    targets: vec![target.clone()],
                    actor: None,
                },
                uuid::Uuid::from_u128(20),
            ),
        )
        .unwrap();
    assert_eq!(plan.targets, vec![target]);
    assert_eq!(plan.recipe.schema, "discord.delete_messages.v1");
    assert_eq!(plan.confirmation.tier, ConfirmationTier::High);
    assert_eq!(plan.restart_policy, RestartPolicy::RequiresNewReview);
    plan.validate().unwrap();
    assert_eq!(
        delete.probes.lock().unwrap().as_slice(),
        &[(CHANNEL.into(), MESSAGE.into(), OWNER.into(), TOKEN.into())]
    );
    assert!(delete.deletes.lock().unwrap().is_empty());
}

#[test]
fn plan_freeze_fails_closed_when_live_message_is_missing_or_not_owned() {
    for (failure, expected) in [
        (DiscordDeleteError::NotFound, ErrorCode::NotFound),
        (
            DiscordDeleteError::OwnershipMismatch,
            ErrorCode::PermissionChanged,
        ),
    ] {
        let (context, record) = fixture();
        let target = content_ref(&record);
        let delete = Arc::new(Delete::rejecting(failure));
        let io = DiscordRemediationIo::new(
            context.clone(),
            OWNER.into(),
            Arc::new(Query(vec![record])),
            ready_session(),
            delete.clone(),
        )
        .unwrap();

        let error = runtime()
            .block_on(
                crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                    &io,
                    &context,
                    crate::providers::ports::PrepareIntent {
                        action_id: "selected_messages".into(),
                        targets: vec![target],
                        actor: None,
                    },
                    uuid::Uuid::from_u128(22),
                ),
            )
            .unwrap_err();

        assert_eq!(error.code, expected);
        assert_eq!(delete.probes.lock().unwrap().len(), 1);
        assert!(delete.deletes.lock().unwrap().is_empty());
    }
}

#[test]
fn archive_content_and_timestamp_must_match_the_live_message_at_plan_freeze() {
    let (context, mut record) = fixture();
    let target = content_ref(&record);
    let live_binding = DiscordLiveMessageBinding::new(
        "synthetic message",
        "2030-01-02T03:04:05.123Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
            .timestamp_millis(),
    );
    record.searchable_text = "attacker-selected description".into();
    let delete = Arc::new(Delete::requiring(live_binding));
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![record])),
        ready_session(),
        delete.clone(),
    )
    .unwrap();

    let error = runtime()
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                &io,
                &context,
                crate::providers::ports::PrepareIntent {
                    action_id: "selected_messages".into(),
                    targets: vec![target],
                    actor: None,
                },
                uuid::Uuid::from_u128(23),
            ),
        )
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::PermissionChanged);
    assert!(delete.probes.lock().unwrap().is_empty());
    assert!(delete.deletes.lock().unwrap().is_empty());
}

#[test]
fn native_reason_binds_owner_source_count_target_and_plan_token() {
    let (context, first) = fixture();
    let second = record_for(
        context.scope.clone(),
        "9007199254741102",
        "1985931830091579394",
    );
    let first_target = content_ref(&first);
    let second_target = content_ref(&second);
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![first, second])),
        ready_session(),
        Arc::new(Delete::default()),
    )
    .unwrap();
    let rt = runtime();
    let plan = rt
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                &io,
                &context,
                crate::providers::ports::PrepareIntent {
                    action_id: "selected_messages".into(),
                    targets: vec![first_target, second_target],
                    actor: None,
                },
                uuid::Uuid::from_u128(30),
            ),
        )
        .unwrap();

    let reason = authorization_reason(&plan).unwrap();

    assert!(reason.contains("Discord plan"), "{reason}");
    assert!(reason.contains(retract_domain::plan_fingerprint_token(&plan.fingerprint).unwrap()));
    assert!(reason.contains("2 reviewed messages"), "{reason}");
    assert!(reason.contains(OWNER), "{reason}");
    assert!(
        reason.contains(&context.scope.source_id.as_uuid().to_string()),
        "{reason}"
    );
    assert!(reason.contains(CHANNEL), "{reason}");
    assert!(reason.contains(MESSAGE), "{reason}");
    assert!(!reason.contains("private content"));
    assert!(!reason.contains(TOKEN));
    assert!(reason.chars().count() <= 256, "{reason}");
}

#[test]
fn distinct_same_count_discord_plans_have_distinct_native_reasons() {
    let (context, first) = fixture();
    let second = record_for(
        context.scope.clone(),
        "9007199254741102",
        "1985931830091579394",
    );
    let first_target = content_ref(&first);
    let second_target = content_ref(&second);
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![first, second])),
        ready_session(),
        Arc::new(Delete::default()),
    )
    .unwrap();
    let rt = runtime();
    let prepare = |target, id| {
        rt.block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                &io,
                &context,
                crate::providers::ports::PrepareIntent {
                    action_id: "selected_messages".into(),
                    targets: vec![target],
                    actor: None,
                },
                id,
            ),
        )
        .unwrap()
    };
    let first_plan = prepare(first_target, uuid::Uuid::from_u128(31));
    let second_plan = prepare(second_target, uuid::Uuid::from_u128(32));

    let first_reason = authorization_reason(&first_plan).unwrap();
    let second_reason = authorization_reason(&second_plan).unwrap();

    assert_ne!(first_reason, second_reason);
    assert!(first_reason.contains("1 reviewed message"));
    assert!(second_reason.contains("1 reviewed message"));
    assert!(
        first_reason
            .contains(retract_domain::plan_fingerprint_token(&first_plan.fingerprint).unwrap())
    );
    assert!(
        second_reason
            .contains(retract_domain::plan_fingerprint_token(&second_plan.fingerprint).unwrap())
    );
}

#[test]
fn mutation_uses_only_frozen_channel_message_and_matching_session_token() {
    let (context, record) = fixture();
    let target = content_ref(&record);
    let delete = Arc::new(Delete::default());
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![record])),
        ready_session(),
        delete.clone(),
    )
    .unwrap();
    let rt = runtime();
    let mut plan = rt
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::describe(
                &io,
                &context,
                crate::providers::ports::PrepareIntent {
                    action_id: "selected_messages".into(),
                    targets: vec![target.clone()],
                    actor: None,
                },
                uuid::Uuid::from_u128(21),
            ),
        )
        .unwrap();
    plan.seal().unwrap();
    rt.block_on(
        crate::providers::frozen_lifecycle::FrozenProviderIo::mutate(
            &io,
            &plan,
            std::slice::from_ref(&target),
            &std::sync::atomic::AtomicBool::new(false),
        ),
    )
    .unwrap();
    assert_eq!(
        delete.deletes.lock().unwrap().as_slice(),
        &[(CHANNEL.into(), MESSAGE.into(), OWNER.into(), TOKEN.into())]
    );

    let error = rt
        .block_on(
            crate::providers::frozen_lifecycle::FrozenProviderIo::mutate(
                &io,
                &plan,
                &[target],
                &std::sync::atomic::AtomicBool::new(true),
            ),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleContext);
    assert_eq!(delete.deletes.lock().unwrap().len(), 1);
}
