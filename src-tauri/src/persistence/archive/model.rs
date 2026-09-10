//! Archive-only structural validation and persisted record encoding.

use std::{
    collections::BTreeMap,
    io::{self, Write},
};

use retract_domain::{
    AccountRecord, ActorRecord, ConnectionState, EvidenceState, ProviderResourceRef, ResourceKind,
    Scope, ScopedResourceRef, SourceKind, SourceRecord, VersionedPayload,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::persistence::{ProviderPayloadValidator, VerifiedNativeAccountIdentity};

use super::ArchiveError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemovalOutcome {
    pub removed_items: u64,
    pub maintenance_pending: bool,
}

#[derive(Clone)]
pub(crate) struct ArchiveSearch {
    pub scope: Scope,
    pub text: String,
    pub kinds: Vec<retract_domain::ContentKind>,
    pub author: Option<retract_domain::ActorId>,
    pub before: Option<chrono::DateTime<chrono::Utc>>,
    pub after: Option<chrono::DateTime<chrono::Utc>>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ImportBatch {
    pub conversations: Vec<retract_domain::ConversationRecord>,
    pub actors: Vec<retract_domain::ActorRecord>,
    pub contents: Vec<retract_domain::ContentRecord>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ImportBatchV2 {
    pub records: ImportBatch,
    pub warnings: Vec<ImportWarningDelta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportWarningCode {
    UnknownConversationKind,
    MissingOptionalContext,
}

impl ImportWarningCode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::UnknownConversationKind => "unknown_conversation_kind",
            Self::MissingOptionalContext => "missing_optional_context",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportWarningDelta {
    pub code: ImportWarningCode,
    pub count: u64,
}

impl ImportBatchV2 {
    pub(crate) fn bounded_size(&self) -> Result<usize, ArchiveError> {
        if self.warnings.len() > MAX_WARNING_CODES {
            return Err(ArchiveError::LimitExceeded);
        }
        for warning in &self.warnings {
            if warning.count == 0 {
                return Err(ArchiveError::InvalidRecord);
            }
            super::ingest_state::integer(warning.count)?;
        }
        if self.records.actors.is_empty()
            && self.records.conversations.is_empty()
            && self.records.contents.is_empty()
        {
            if self.warnings.is_empty() {
                return Err(ArchiveError::InvalidRecord);
            }
        } else {
            self.records.bounded_size()?;
        }
        encoded_size(
            &batch_v2_payload(&self.records, &self.warnings),
            MAX_BATCH_BYTES,
        )
    }

    pub(super) fn normalize_provider_fields(&mut self) {
        for content in &mut self.records.contents {
            clear_derived_fields(content);
        }
    }
}

pub(super) fn clear_derived_fields(content: &mut retract_domain::ContentRecord) {
    content.privacy_findings.clear();
    content.detector_version = None;
}

pub(super) fn batch_v2_payload<'a>(
    records: &'a ImportBatch,
    warnings: &'a [ImportWarningDelta],
) -> impl Serialize + 'a {
    // Domain separation and an explicit version ensure v1 digests remain valid
    // only under their legacy receipt format. Array order is intentionally kept.
    ("retract.archive.batch", 2_u8, records, warnings)
}

/// Backend input only: provider payloads are untrusted and the store owns all
/// account/source UUIDs and canonical identity resolution. No mutation grant.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct NewArchiveImport {
    pub provider: retract_domain::ProviderKey,
    pub native_identity: VersionedPayload,
    pub display_name: String,
    pub username: Option<String>,
    pub avatar: Option<VersionedPayload>,
    pub fingerprint: String,
    pub schema_profile: VersionedPayload,
    pub parser_policy: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

impl NewArchiveImport {
    pub(super) fn bounded_size(&self) -> Result<(), ArchiveError> {
        encoded_size(&self.native_identity, ENVELOPE_BYTES)?;
        envelope_bounds(self.avatar.as_ref())?;
        provenance_bounds(&self.fingerprint, &self.schema_profile)?;
        if self.parser_policy.trim().is_empty()
            || self.parser_policy.len() > 256
            || self.parser_policy.chars().any(char::is_control)
        {
            return Err(ArchiveError::InvalidRecord);
        }
        encoded_size(self, MAX_BATCH_BYTES)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportDisposition {
    Start,
    Ready,
    Busy,
    RetryRequired,
}

#[derive(Debug)]
pub(crate) struct ArchiveImportResolution {
    pub account: AccountRecord,
    pub source: SourceRecord,
    pub checkpoint: ImportCheckpoint,
    pub disposition: ImportDisposition,
    pub session: Option<std::sync::Arc<ImportSession>>,
}

#[derive(Debug)]
pub(crate) struct ImportSession {
    pub(super) id: uuid::Uuid,
    pub(super) scope: retract_domain::Scope,
    pub(super) fingerprint: String,
    pub(super) schema_profile: retract_domain::VersionedPayload,
    pub(super) cancellation: ImportCancellation,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ImportCancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl ImportCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    pub(super) fn check(&self) -> Result<(), ArchiveError> {
        if self.0.load(std::sync::atomic::Ordering::Acquire) {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl ImportSession {
    pub(crate) fn cancellation_signal(&self) -> ImportCancellation {
        self.cancellation.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum ImportPhase {
    Importing,
    Ready,
    Interrupted,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportProgress {
    pub phase: ImportPhase,
    pub committed_items: u64,
    pub committed_bytes: u64,
    pub next_batch: u64,
}

/// Read-only checkpoint, never a deserializable mutation grant. The revision
/// changes on every transition, including a retry with no accepted new input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportCheckpoint {
    pub scope: Scope,
    pub fingerprint: String,
    pub schema_profile: VersionedPayload,
    pub run_id: uuid::Uuid,
    pub revision: u64,
    pub progress: ImportProgress,
    pub warnings: Vec<ImportWarning>,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub failure_code: Option<ImportFailureCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportFailureCode {
    InvalidArchive,
    UnsupportedProfile,
    LimitExceeded,
    InputChanged,
    IncompleteSource,
    StorageFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportWarning {
    pub code: String,
    pub count: u64,
}

pub(crate) const MAX_BATCH_RECORDS: usize = 500;
pub(crate) const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_SEARCHABLE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_ATTACHMENTS: usize = 100;
pub(crate) const MAX_WARNING_CODES: usize = 32;
pub(crate) const MAX_QUEUED_BATCHES: usize = 2;

#[derive(Clone, Copy)]
pub(super) struct ImportLimits {
    pub items: u64,
    pub bytes: u64,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            items: 1_000_000,
            bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

impl ImportBatch {
    /// Worker callers can reject a batch before copying or queueing it. No
    /// encoded buffer or detector input is allocated until these bounds pass.
    pub(crate) fn bounded_size(&self) -> Result<usize, ArchiveError> {
        let count = self.conversations.iter().try_fold(
            self.contents
                .len()
                .checked_add(self.actors.len())
                .ok_or(ArchiveError::LimitExceeded)?,
            |total, conversation| {
                total
                    .checked_add(1)
                    .and_then(|total| total.checked_add(conversation.participants.len()))
                    .ok_or(ArchiveError::LimitExceeded)
            },
        )?;
        if count == 0 {
            return Err(ArchiveError::InvalidRecord);
        }
        if count > MAX_BATCH_RECORDS {
            return Err(ArchiveError::LimitExceeded);
        }
        self.item_bounds()?;
        encoded_size(self, MAX_BATCH_BYTES)
    }

    fn item_bounds(&self) -> Result<(), ArchiveError> {
        for actor in self
            .actors
            .iter()
            .chain(self.conversations.iter().flat_map(|c| &c.participants))
        {
            resource_bounds(&actor.resource)?;
            envelope_bounds(actor.avatar.as_ref())?;
        }
        for conversation in &self.conversations {
            resource_bounds(&conversation.resource)?;
            envelope_bounds(conversation.provider_metadata.as_ref())?;
        }
        for content in &self.contents {
            if content.attachments.len() > MAX_ATTACHMENTS {
                return Err(ArchiveError::LimitExceeded);
            }
            let searchable_bytes = content.attachments.iter().try_fold(
                content.searchable_text.len(),
                |bytes, attachment| {
                    bytes
                        .checked_add(attachment.safe_display_name.as_ref().map_or(0, String::len))
                        .ok_or(ArchiveError::LimitExceeded)
                },
            )?;
            if searchable_bytes > MAX_SEARCHABLE_BYTES {
                return Err(ArchiveError::LimitExceeded);
            }
            resource_bounds(&content.resource)?;
            envelope_bounds(content.provider_metadata.as_ref())?;
            for attachment in &content.attachments {
                envelope_bounds(Some(&attachment.locator))?;
            }
        }
        Ok(())
    }

    pub(super) fn digest(&self) -> Result<String, ArchiveError> {
        digest_serialized(self)
    }

    pub(super) fn validate(
        &self,
        scope: &Scope,
        validator: &dyn ProviderPayloadValidator,
    ) -> Result<(), ArchiveError> {
        let mut actors = BTreeMap::new();
        for actor in self
            .actors
            .iter()
            .chain(self.conversations.iter().flat_map(|c| &c.participants))
        {
            validate_actor(actor, scope, validator)?;
            if actors
                .insert(actor.id, actor)
                .is_some_and(|previous| previous != actor)
            {
                return Err(ArchiveError::InvalidRecord);
            }
        }
        let mut conversations = BTreeMap::new();
        for conversation in &self.conversations {
            if conversation.evidence != EvidenceState::Archive {
                return Err(ArchiveError::InvalidRecord);
            }
            validate_resource(&conversation.resource, validator)?;
            validate_envelope(conversation.provider_metadata.as_ref())?;
            conversation
                .validate(scope)
                .map_err(|_| ArchiveError::InvalidRecord)?;
            validator
                .validate_archive_conversation(conversation)
                .map_err(|_| ArchiveError::InvalidRecord)?;
            if conversations
                .insert(conversation.id, conversation)
                .is_some_and(|previous| previous != conversation)
            {
                return Err(ArchiveError::InvalidRecord);
            }
        }
        let mut contents = BTreeMap::new();
        for content in &self.contents {
            if content.evidence != EvidenceState::Archive {
                return Err(ArchiveError::InvalidRecord);
            }
            validate_resource(&content.resource, validator)?;
            validate_envelope(content.provider_metadata.as_ref())?;
            for attachment in &content.attachments {
                validate_envelope(Some(&attachment.locator))?;
            }
            content
                .validate(scope)
                .map_err(|_| ArchiveError::InvalidRecord)?;
            validator
                .validate_archive_content(content)
                .map_err(|_| ArchiveError::InvalidRecord)?;
            if contents
                .insert(content.id, content)
                .is_some_and(|previous| previous != content)
            {
                return Err(ArchiveError::InvalidRecord);
            }
        }
        Ok(())
    }
}

pub(super) fn validate_resource(
    resource: &ProviderResourceRef,
    validator: &dyn ProviderPayloadValidator,
) -> Result<(), ArchiveError> {
    resource_bounds(resource)?;
    resource
        .validate()
        .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_resource(resource)
        .map_err(|_| ArchiveError::InvalidRecord)
}

fn validate_actor(
    actor: &ActorRecord,
    scope: &Scope,
    validator: &dyn ProviderPayloadValidator,
) -> Result<(), ArchiveError> {
    if actor.evidence != EvidenceState::Archive
        || actor.resource.resource_kind != ResourceKind::Actor
    {
        return Err(ArchiveError::InvalidRecord);
    }
    validate_resource(&actor.resource, validator)?;
    validate_envelope(actor.avatar.as_ref())?;
    ScopedResourceRef {
        id: *actor.id.as_uuid(),
        scope: actor.scope.clone(),
        resource: actor.resource.clone(),
    }
    .validate(scope)
    .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_archive_actor(actor)
        .map_err(|_| ArchiveError::InvalidRecord)
}

fn validate_envelope(envelope: Option<&VersionedPayload>) -> Result<(), ArchiveError> {
    envelope_bounds(envelope)?;
    if let Some(envelope) = envelope {
        envelope
            .validate()
            .map_err(|_| ArchiveError::InvalidRecord)?;
    }
    Ok(())
}

fn resource_bounds(resource: &ProviderResourceRef) -> Result<(), ArchiveError> {
    encoded_size(resource, ENVELOPE_BYTES).map(|_| ())
}

fn envelope_bounds(envelope: Option<&VersionedPayload>) -> Result<(), ArchiveError> {
    if let Some(envelope) = envelope {
        encoded_size(envelope, ENVELOPE_BYTES)?;
    }
    Ok(())
}

/// Source fingerprint changes whenever the shared detector implementation does.
/// No provider, regex, or detection implementation is added to retract-domain.
pub(super) fn derive_findings(content: &mut retract_domain::ContentRecord) {
    use cleaner_domain::{ContentKind as LegacyKind, SensitiveDataKind as Finding};
    use retract_domain::{ContentKind, PrivacyKind};
    fn kind(value: ContentKind) -> LegacyKind {
        match value {
            ContentKind::Text => LegacyKind::Text,
            ContentKind::Image => LegacyKind::Photo,
            ContentKind::Video => LegacyKind::Video,
            ContentKind::Document => LegacyKind::File,
            ContentKind::Voice => LegacyKind::Voice,
            ContentKind::Audio => LegacyKind::Audio,
            ContentKind::Animation => LegacyKind::Animation,
            ContentKind::Sticker => LegacyKind::Sticker,
            ContentKind::Poll => LegacyKind::Poll,
            ContentKind::Location => LegacyKind::Location,
            ContentKind::Contact => LegacyKind::Contact,
            ContentKind::Service => LegacyKind::Service,
            ContentKind::Other => LegacyKind::Other,
        }
    }
    let mut findings = std::collections::BTreeSet::new();
    for finding in
        cleaner_domain::detect_sensitive_data(&content.searchable_text, kind(content.kind))
            .into_iter()
            .chain(content.attachments.iter().flat_map(|attachment| {
                cleaner_domain::detect_sensitive_data(
                    attachment.safe_display_name.as_deref().unwrap_or(""),
                    kind(attachment.kind),
                )
            }))
    {
        findings.insert(match finding {
            Finding::EmailAddress => PrivacyKind::EmailAddress,
            Finding::PhoneNumber => PrivacyKind::PhoneNumber,
            Finding::PostalAddress => PrivacyKind::PostalAddress,
            Finding::PreciseLocation => PrivacyKind::PreciseLocation,
            Finding::PersonalIdentifier => PrivacyKind::PersonalIdentifier,
            Finding::IdentityDocument => PrivacyKind::IdentityDocument,
            Finding::FinancialAccount => PrivacyKind::FinancialAccount,
            Finding::CryptoWallet => PrivacyKind::CryptoWallet,
            Finding::CredentialOrSecret => PrivacyKind::CredentialOrSecret,
            Finding::NetworkAddress => PrivacyKind::NetworkAddress,
            Finding::ContactCard => PrivacyKind::ContactCard,
        });
    }
    content.privacy_findings = findings.into_iter().collect();
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "cleaner-sha256:{}",
            hex(&Sha256::digest(include_bytes!(
                "../../../../crates/cleaner-domain/src/sensitive.rs"
            )))
        )
    });
    content.detector_version = Some(VERSION.clone());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn digest_serialized(value: &impl Serialize) -> Result<String, ArchiveError> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).map_err(|_| ArchiveError::InvalidRecord)?;
    Ok(hex(&writer.0.finalize()))
}

pub(crate) const ENVELOPE_BYTES: usize = 64 * 1024;

pub(super) fn registration_bounds(
    account: &AccountRecord,
    source: &SourceRecord,
) -> Result<(), ArchiveError> {
    if source.warnings.len() > MAX_WARNING_CODES {
        return Err(ArchiveError::LimitExceeded);
    }
    validate_account_envelopes(account)?;
    encoded_size(&source.schema_profile, ENVELOPE_BYTES)?;
    encoded_size(&(account, source), MAX_BATCH_BYTES)?;
    Ok(())
}

pub(super) fn provenance_bounds(
    fingerprint: &str,
    schema: &VersionedPayload,
) -> Result<(), ArchiveError> {
    encoded_size(schema, ENVELOPE_BYTES)?;
    encoded_size(&(fingerprint, schema), MAX_BATCH_BYTES)?;
    Ok(())
}

pub(super) fn checkpoint_bounds(checkpoint: &ImportCheckpoint) -> Result<(), ArchiveError> {
    if checkpoint.warnings.len() > MAX_WARNING_CODES {
        return Err(ArchiveError::LimitExceeded);
    }
    provenance_bounds(&checkpoint.fingerprint, &checkpoint.schema_profile)?;
    encoded_size(checkpoint, MAX_BATCH_BYTES)?;
    Ok(())
}

/// Counts the actual JSON encoding without retaining a serialized buffer and
/// aborts serialization as soon as the approved byte ceiling is exceeded.
pub(crate) fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize, ArchiveError> {
    struct Counter {
        bytes: usize,
        limit: usize,
        exceeded: bool,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit - self.bytes {
                self.exceeded = true;
                return Err(io::Error::other("archive encoded size limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        limit,
        exceeded: false,
    };
    serde_json::to_writer(&mut counter, value).map_err(|_| {
        if counter.exceeded {
            ArchiveError::LimitExceeded
        } else {
            ArchiveError::InvalidRecord
        }
    })?;
    Ok(counter.bytes)
}

pub(super) fn validate_account_envelopes(account: &AccountRecord) -> Result<(), ArchiveError> {
    encoded_size(&account.native_identity, ENVELOPE_BYTES)?;
    if let Some(avatar) = &account.avatar {
        encoded_size(avatar, ENVELOPE_BYTES)?;
    }
    Ok(())
}

pub(super) fn validate_archive_account(
    account: &AccountRecord,
    validator: &dyn ProviderPayloadValidator,
) -> Result<VerifiedNativeAccountIdentity, ArchiveError> {
    if account.connection_state != ConnectionState::Disconnected {
        return Err(ArchiveError::InvalidRecord);
    }
    validate_account_envelopes(account)?;
    account
        .validate()
        .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_account(account)
        .map_err(|_| ArchiveError::InvalidRecord)
}

pub(super) fn validate_registration(
    account: &AccountRecord,
    source: &SourceRecord,
    validator: &dyn ProviderPayloadValidator,
) -> Result<VerifiedNativeAccountIdentity, ArchiveError> {
    if source.provider != account.provider || source.account_id != account.id {
        return Err(ArchiveError::ScopeMismatch);
    }
    if source.kind != SourceKind::ArchiveImport {
        return Err(ArchiveError::InvalidRecord);
    }
    let native = validate_archive_account(account, validator)?;
    encoded_size(&source.schema_profile, ENVELOPE_BYTES)?;
    source
        .validate(account)
        .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_source(source, account)
        .map_err(|_| ArchiveError::InvalidRecord)?;
    Ok(native)
}

pub(super) fn encode(record: &impl Serialize) -> Result<String, ArchiveError> {
    serde_json::to_string(record).map_err(|_| ArchiveError::InvalidRecord)
}

pub(super) fn decode<T: DeserializeOwned>(record: &str) -> Result<T, ArchiveError> {
    serde_json::from_str(record).map_err(|_| ArchiveError::InvalidRecord)
}
