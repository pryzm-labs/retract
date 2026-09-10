//! Application-lifetime registration of lazy archive work and terminal drain.
use super::{ArchiveError, ArchiveService, ArchiveStore};
use std::sync::Arc;
use tokio::sync::Mutex;

type Opener = dyn Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync;

fn application_validators() -> std::collections::BTreeMap<
    retract_domain::ProviderKey,
    Arc<dyn crate::persistence::ProviderPayloadValidator>,
> {
    use crate::providers::discord::{DiscordPayloadValidator, locators::discord_provider_key};
    std::collections::BTreeMap::from([(
        discord_provider_key(),
        Arc::new(DiscordPayloadValidator) as Arc<dyn crate::persistence::ProviderPayloadValidator>,
    )])
}
pub(crate) struct ArchiveOwner {
    opener: Arc<Opener>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    service: Option<Arc<ArchiveService>>,
}

impl ArchiveOwner {
    pub(crate) fn unavailable() -> Self {
        Self::with_opener(|| Err(ArchiveError::UnavailableKey))
    }
    pub(super) fn with_opener(
        opener: impl Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            opener: Arc::new(opener),
            state: Mutex::new(State::default()),
        }
    }

    pub(crate) fn application(root: std::path::PathBuf) -> Self {
        let validators = application_validators();
        Self::with_opener(move || {
            #[cfg(target_os = "macos")]
            {
                let path = prepare_archive_directory(&root)?.join("content.db");
                ArchiveStore::open_with_key_loader(path, validators.clone(), || {
                    crate::secure_store::load_archive_index_key().map_err(|error| match error {
                        crate::error::AppError::ProfileInUse => ArchiveError::StoreInUse,
                        _ => ArchiveError::UnavailableKey,
                    })
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (&root, &validators);
                Err(ArchiveError::UnavailableKey)
            }
        })
    }

    /// First-open failures are sticky to avoid repeated credential prompts/I/O.
    /// Correct the condition and restart the application; import retry and Busy
    /// backoff do not reset this application-lifetime factory.
    pub(crate) async fn open(&self) -> Result<Arc<ArchiveService>, ArchiveError> {
        let mut lifecycle = self.state.lock().await;
        if lifecycle.closed {
            return Err(ArchiveError::Cancelled);
        }
        let service = lifecycle.service.get_or_insert_with(|| {
            let factory = self.opener.clone();
            ArchiveService::start(move || factory())
        });
        service.wait_ready().await?;
        Ok(Arc::clone(service))
    }

    pub(crate) async fn shutdown(&self) {
        let mut lifecycle = self.state.lock().await;
        lifecycle.closed = true;
        if let Some(service) = lifecycle.service.as_ref() {
            service.shutdown().await;
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn prepare_archive_directory(
    root: &std::path::Path,
) -> Result<std::path::PathBuf, ArchiveError> {
    use std::{fs, io::ErrorKind, os::unix::fs::DirBuilderExt};
    // Create only beneath an existing canonical, non-writable parent; all work
    // runs on the worker. Never relax the repository's existing parent checks.
    if let Err(error) = fs::symlink_metadata(root) {
        if error.kind() != ErrorKind::NotFound {
            return Err(ArchiveError::InvalidStore);
        }
        super::store::validate_parent(root)?;
        match fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ArchiveError::StorageFailure),
        }
    }
    super::store::validate_parent(&root.join("archives"))?;
    let directory = root.join("archives");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(ArchiveError::StorageFailure),
    }
    super::store::validate_parent(&directory.join("content.db"))?;
    Ok(directory)
}

#[cfg(test)]
mod discord_tests {
    use super::*;
    use crate::persistence::archive::{ArchiveKey, ArchiveQuerySource, ImportBatch};
    use crate::providers::discord::{
        DiscordNormalizer, locators::discord_provider_key, model::DiscordSourceProfile,
    };
    use crate::providers::ports::{ContentQuery, QuerySource};
    use discord_archive::{ChannelContext, DiscordId, ExportAccount, SentMessage};
    use retract_domain::*;

    #[test]
    fn discord_application_validator_registration_is_lazy_and_does_not_offer_a_provider() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("absent");
        let owner = ArchiveOwner::application(root.clone());
        let state = owner.state.try_lock().unwrap();
        assert!(state.service.is_none()); // No worker/opener can access the DB or key loader.
        assert!(!root.exists());
        let validators = application_validators();
        assert_eq!(validators.len(), 1);
        assert_eq!(
            validators[&discord_provider_key()]
                .validation_policy_key()
                .as_str(),
            "discord.archive_payload.v1"
        );
        assert!(
            crate::providers::ProviderRegistry::default()
                .get(&discord_provider_key())
                .is_err()
        );
    }

    #[test]
    fn discord_archive_reopens_source_observations_and_derives_shared_privacy_findings() {
        let directory = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(directory.path())
            .unwrap()
            .join("content.db");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let observed = "2031-01-02T03:04:05.123Z".parse().unwrap();
            let account_input = ExportAccount {
                id: DiscordId::parse("9007199254741001").unwrap(),
                username: "invented_owner".into(),
            };
            let channel = ChannelContext {
                id: DiscordId::parse("9007199254741101").unwrap(),
                source_type: "opaque".into(),
                name: Some("Invented room".into()),
                recipients: None,
                guild: None,
            };
            let mut message = SentMessage {
                id: DiscordId::parse("1985931830091579393").unwrap(),
                account_id: account_input.id.clone(),
                channel_id: channel.id.clone(),
                timestamp_millis: 1_893_553_445_123,
                contents: "owner@example.test".into(),
                attachments: "https://example.invalid/passport.png".into(),
            };
            let make_owner = || {
                let path = path.clone();
                ArchiveOwner::with_opener(move || {
                    ArchiveStore::open(
                        path.clone(),
                        ArchiveKey::new([0x85; 32]),
                        application_validators(),
                    )
                })
            };
            let owner = make_owner();
            let service = owner.open().await.unwrap();
            let mut expected = vec![];
            for source_number in [2, 3] {
                let scope = Scope {
                    provider: discord_provider_key(),
                    account_id: uuid::Uuid::from_u128(1).try_into().unwrap(),
                    source_id: uuid::Uuid::from_u128(source_number).try_into().unwrap(),
                };
                let normalizer = DiscordNormalizer::new(scope.clone(), observed).unwrap();
                let account = normalizer.account(&account_input).unwrap();
                let source = SourceRecord {
                    id: scope.source_id,
                    account_id: scope.account_id,
                    provider: discord_provider_key(),
                    kind: SourceKind::ArchiveImport,
                    state: SourceState::Preparing,
                    archive_fingerprint: Some(format!("synthetic-{source_number}")),
                    schema_profile: DiscordSourceProfile::payload(),
                    imported_at: None,
                    updated_at: observed,
                    warnings: vec![],
                };
                let record = normalizer
                    .content(&account_input, &channel, &message)
                    .unwrap();
                service.register_source(&account, &source).await.unwrap();
                let session = service.begin_import(&scope).await.unwrap();
                service
                    .append_batch(
                        &session,
                        0,
                        &ImportBatch {
                            conversations: vec![normalizer.conversation(&channel).unwrap()],
                            actors: vec![normalizer.actor(&account_input).unwrap()],
                            contents: vec![record.clone()],
                        },
                    )
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                service.finish_import(&session).await.unwrap();
                expected.push(record);
                message.contents = "new observation".into();
                message.attachments = "https://example.invalid/second.png".into();
            }
            owner.shutdown().await;
            let owner = make_owner();
            let service = owner.open().await.unwrap();
            let query = ArchiveQuerySource(service.clone());
            for (index, expected) in expected.iter().enumerate() {
                let found = query
                    .search(ContentQuery {
                        scope: expected.scope.clone(),
                        query: String::new(),
                        cursor: None,
                        limit: 200,
                    })
                    .await
                    .unwrap()
                    .items;
                assert_eq!(found.len(), 1);
                let mut record = found[0].clone();
                if index == 0 {
                    assert!(record.privacy_findings.contains(&PrivacyKind::EmailAddress));
                    assert!(
                        record
                            .privacy_findings
                            .contains(&PrivacyKind::IdentityDocument)
                    );
                } else {
                    assert!(record.privacy_findings.is_empty());
                }
                assert!(
                    record
                        .detector_version
                        .as_deref()
                        .unwrap()
                        .starts_with("cleaner-sha256:")
                );
                record.privacy_findings.clear();
                record.detector_version = None;
                assert_eq!(&record, expected);
            }
            assert_eq!(expected[0].id, expected[1].id);
            assert_eq!(expected[0].resource, expected[1].resource);
            assert_eq!(
                service
                    .remove_source(&expected[0].scope)
                    .await
                    .unwrap()
                    .removed_items,
                1
            );
            assert_eq!(
                query
                    .search(ContentQuery {
                        scope: expected[1].scope.clone(),
                        query: "new observation".into(),
                        cursor: None,
                        limit: 200
                    })
                    .await
                    .unwrap()
                    .items
                    .len(),
                1
            );
            owner.shutdown().await;
        });
    }
}
