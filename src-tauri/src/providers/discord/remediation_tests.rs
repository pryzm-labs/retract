use super::remediation::{DiscordMessageDelete, DiscordRemediationIo};
use super::session::{
    DiscordCredentialStore, DiscordIdentityClient, DiscordSessionOwner, StoredDiscordCredential,
    VerifiedDiscordIdentity,
};
use super::{DiscordNormalizer, http::DeleteOutcome};
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
struct Delete(Mutex<Vec<(String, String, String)>>);
#[async_trait]
impl DiscordMessageDelete for Delete {
    async fn delete(
        &self,
        channel: &str,
        message: &str,
        token: &str,
        _: &AtomicBool,
    ) -> Result<DeleteOutcome, super::http::DiscordDeleteError> {
        self.0
            .lock()
            .unwrap()
            .push((channel.into(), message.into(), token.into()));
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
    let io = DiscordRemediationIo::new(
        context.clone(),
        OWNER.into(),
        Arc::new(Query(vec![record])),
        ready_session(),
        Arc::new(Delete::default()),
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
    assert_eq!(plan.restart_policy, RestartPolicy::ResumeFrozenTargets);
    plan.validate().unwrap();
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
        crate::providers::frozen_lifecycle::FrozenProviderIo::mutate(&io, &plan, &[target]),
    )
    .unwrap();
    assert_eq!(
        delete.0.lock().unwrap().as_slice(),
        &[(CHANNEL.into(), MESSAGE.into(), TOKEN.into())]
    );
}
