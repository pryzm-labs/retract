use super::{key, parser, profile, resource, user};
use crate::persistence::archive::*;
use crate::providers::archive_file::FileIdentity;
use chrono::{DateTime, Utc};
use discord_archive::{ArchiveInventory, ArchiveLimits, Cancellation};
use retract_domain::*;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

struct Cancel<'a>(&'a AtomicBool);
impl Cancellation for Cancel<'_> {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
impl Cancel<'_> {
    fn check(&self) -> Result<(), ArchiveError> {
        if self.is_cancelled() {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }
}
pub(crate) fn import_file(
    mut file: File,
    owner: &ArchiveOwner,
    cancelled: &AtomicBool,
) -> Result<Scope, ArchiveError> {
    let cancel = Cancel(cancelled);
    cancel.check()?;
    let initial = FileIdentity::read(&file).ok_or(ArchiveError::InvalidRecord)?;
    let limits = ArchiveLimits {
        max_archive_bytes: 256 * 1024 * 1024,
        max_observed_bytes: 512 * 1024 * 1024,
        ..ArchiveLimits::default()
    };
    if initial.size > limits.max_archive_bytes {
        return Err(ArchiveError::LimitExceeded);
    }
    let fingerprint = digest_file(&mut file, initial.size, &cancel)?;
    let parse_error = |error| match error {
        discord_archive::ArchiveError::Cancelled => ArchiveError::Cancelled,
        discord_archive::ArchiveError::LimitExceeded => ArchiveError::LimitExceeded,
        _ => ArchiveError::InvalidRecord,
    };
    let mut inventory =
        ArchiveInventory::inspect(&mut file, limits, &cancel).map_err(parse_error)?;
    let parser::Layout { account, parts } = parser::inspect(&mut inventory).map_err(parse_error)?;
    let multipart = parts.len() > 1;
    let runtime = tokio::runtime::Handle::current();
    let service = runtime.block_on(owner.open())?;
    let resolution = runtime.block_on(
        service.resolve_or_register_import(NewArchiveImport {
            provider: key(),
            native_identity: user(&account.id),
            display_name: account.username.clone(),
            username: Some(account.username.clone()),
            avatar: None,
            fingerprint: fingerprint.clone(),
            schema_profile: profile(multipart),
            parser_policy: if multipart {
                "x.multipart.v1"
            } else {
                "x.coordinator.v2"
            }
            .into(),
            observed_at: Utc::now(),
        }),
    )?;
    let scope = resolution.checkpoint.scope.clone();
    let session = match resolution.disposition {
        ImportDisposition::Ready => {
            drop(inventory);
            cancel.check()?;
            initial.verify(&file).ok_or(ArchiveError::InvalidRecord)?;
            return Ok(scope);
        }
        ImportDisposition::Busy => return Err(ArchiveError::Busy),
        ImportDisposition::RetryRequired => {
            runtime.block_on(service.retry_import(&resolution.checkpoint))?
        }
        ImportDisposition::Start => resolution.session.ok_or(ArchiveError::StorageFailure)?,
    };
    let observed_at = resolution.checkpoint.observed_at;
    let staged = (|| {
        let actor = resource(&scope, ResourceKind::Actor, &account.id);
        let feed = resource(&scope, ResourceKind::Conversation, &account.id);
        let invalid = |_| ArchiveError::InvalidRecord;
        let author_id = actor
            .resource_id()
            .map_err(invalid)?
            .try_into()
            .map_err(invalid)?;
        let conversation_id = feed
            .resource_id()
            .map_err(invalid)?
            .try_into()
            .map_err(invalid)?;
        let mut batch = ImportBatchV2::default();
        batch.records.actors.push(ActorRecord {
            id: author_id,
            scope: scope.clone(),
            resource: actor,
            display_name: account.username.clone(),
            username: Some(account.username),
            avatar: None,
            evidence: EvidenceState::Archive,
            observed_at,
        });
        batch.records.conversations.push(ConversationRecord {
            id: conversation_id,
            scope: scope.clone(),
            resource: feed,
            kind: ConversationKind::AccountFeed,
            title: "Archived posts".into(),
            parent_id: None,
            participant_count: None,
            participants: vec![],
            evidence: EvidenceState::Archive,
            observed_at,
            provider_metadata: None,
        });
        let mut sequence = 0;
        let mut append = |batch: &ImportBatchV2| -> Result<(), ArchiveError> {
            loop {
                cancel.check()?;
                match service.append_batch_v2(&session, sequence, batch) {
                    Ok(reply) => {
                        reply
                            .blocking_recv()
                            .map_err(|_| ArchiveError::StorageFailure)??;
                        sequence += 1;
                        return Ok(());
                    }
                    Err(ArchiveError::Busy) => {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        let mut store_post = |post: parser::Post| -> Result<(), ArchiveError> {
            cancel.check()?;
            let reference = resource(&scope, ResourceKind::Content, &post.id_str);
            let timestamp = DateTime::parse_from_str(&post.created_at, "%a %b %d %H:%M:%S %z %Y")
                .map_err(|_| ArchiveError::InvalidRecord)?
                .with_timezone(&Utc);
            batch.records.contents.push(ContentRecord {
                id: reference
                    .resource_id()
                    .map_err(invalid)?
                    .try_into()
                    .map_err(invalid)?,
                scope: scope.clone(),
                conversation_id,
                resource: reference,
                author_id,
                timestamp,
                edited_at: None,
                kind: ContentKind::Text,
                searchable_text: post.full_text,
                attachments: vec![],
                reply_to: None,
                thread_parent: None,
                external_location: ExternalLocationAvailability::Unsupported,
                evidence: EvidenceState::Archive,
                observed_at,
                privacy_findings: vec![],
                detector_version: None,
                provider_metadata: None,
            });
            if batch.records.contents.len() == 16 {
                append(&batch)?;
                batch = ImportBatchV2::default();
            }
            Ok(())
        };
        // Preserve storage/cancellation errors across the inventory's parser-only callback.
        let mut sink_error = None;
        let parsed = parser::visit_posts(&mut inventory, &parts, |post| {
            store_post(post).map_err(|error| {
                sink_error = Some(error);
                discord_archive::ArchiveError::ReadFailure
            })
        });
        parsed.map_err(|error| sink_error.unwrap_or_else(|| parse_error(error)))?;
        if !batch.records.contents.is_empty() || !batch.records.actors.is_empty() {
            append(&batch)?;
        }
        Ok(())
    })();
    drop(inventory);
    let result = staged.and_then(|()| {
        cancel.check()?;
        if digest_file(&mut file, initial.size, &cancel)? != fingerprint
            || initial.verify(&file).is_none()
        {
            return Err(ArchiveError::InvalidRecord);
        }
        runtime.block_on(service.finish_import(&session))?;
        Ok(scope)
    });
    if let Err(error) = result {
        if error == ArchiveError::Cancelled {
            runtime.block_on(service.cancel_import(&session))?;
        } else {
            let failure = match error {
                ArchiveError::InvalidRecord | ArchiveError::ConflictingObservation => {
                    ImportFailureCode::InvalidArchive
                }
                ArchiveError::LimitExceeded => ImportFailureCode::LimitExceeded,
                _ => ImportFailureCode::StorageFailure,
            };
            runtime.block_on(service.fail_import(&session, failure))?;
        }
    }
    result
}

fn digest_file(file: &mut File, size: u64, cancel: &Cancel) -> Result<String, ArchiveError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ArchiveError::InvalidRecord)?;
    let mut digest = Sha256::new();
    let mut bytes = [0; 65536];
    let mut count = 0u64;
    loop {
        cancel.check()?;
        let n = file
            .read(&mut bytes)
            .map_err(|_| ArchiveError::InvalidRecord)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > size {
            return Err(ArchiveError::InvalidRecord);
        }
        digest.update(&bytes[..n]);
    }
    if count != size {
        return Err(ArchiveError::InvalidRecord);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
