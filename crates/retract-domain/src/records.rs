//! Transient normalized records. Provider-specific metadata remains opaque;
//! its typed representation and interpretation belong to the owning adapter.

use crate::{
    AccountId, ActorId, ContentId, ConversationId, DomainError, ProviderKey, ProviderResourceRef,
    ResourceKind, SafeError, Scope, ScopedResourceRef, SourceId, VersionedPayload,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    AuthenticationRequired,
    Ready,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    LiveConnection,
    ArchiveImport,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Preparing,
    Ready,
    Unavailable,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Live,
    Archive,
    LiveAndArchive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Direct,
    Group,
    CommunityChannel,
    Broadcast,
    PublicThread,
    PrivateThread,
    AccountFeed,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Image,
    Video,
    Document,
    Voice,
    Audio,
    Animation,
    Sticker,
    Poll,
    Location,
    Contact,
    Service,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalLocationAvailability {
    Available,
    Unavailable,
    Unsupported,
}

/// Same categories as the existing detector. The adapter translates findings;
/// this crate contains neither detector logic nor a legacy-domain dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyKind {
    EmailAddress,
    PhoneNumber,
    PostalAddress,
    PreciseLocation,
    PersonalIdentifier,
    IdentityDocument,
    FinancialAccount,
    CryptoWallet,
    CredentialOrSecret,
    NetworkAddress,
    ContactCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountRecord {
    pub id: AccountId,
    pub provider: ProviderKey,
    pub native_identity: VersionedPayload,
    pub display_name: String,
    pub username: Option<String>,
    pub avatar: Option<VersionedPayload>,
    pub connection_state: ConnectionState,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}
impl AccountRecord {
    pub fn validate(&self) -> Result<(), DomainError> {
        self.native_identity.validate()?;
        if let Some(avatar) = &self.avatar {
            avatar.validate()?;
        }
        if self.last_seen_at < self.created_at {
            return Err(DomainError::InvalidReference);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRecord {
    pub id: SourceId,
    pub account_id: AccountId,
    pub provider: ProviderKey,
    pub kind: SourceKind,
    pub state: SourceState,
    pub archive_fingerprint: Option<String>,
    pub schema_profile: VersionedPayload,
    pub imported_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub warnings: Vec<SafeError>,
}
impl SourceRecord {
    pub fn validate(&self, account: &AccountRecord) -> Result<(), DomainError> {
        account.validate()?;
        if self.account_id != account.id || self.provider != account.provider {
            return Err(DomainError::ScopeMismatch);
        }
        self.schema_profile.validate()?;
        match self.kind {
            SourceKind::LiveConnection
                if self.archive_fingerprint.is_some() || self.imported_at.is_some() =>
            {
                return Err(DomainError::InvalidReference);
            }
            SourceKind::ArchiveImport
                if self
                    .archive_fingerprint
                    .as_ref()
                    .is_none_or(|s| !crate::identity::bounded_text(s, 256)) =>
            {
                return Err(DomainError::InvalidReference);
            }
            _ => (),
        }
        Ok(())
    }
    pub fn scope(&self) -> Scope {
        Scope {
            provider: self.provider.clone(),
            account_id: self.account_id,
            source_id: self.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorRecord {
    pub id: ActorId,
    pub scope: Scope,
    pub resource: ProviderResourceRef,
    pub display_name: String,
    pub username: Option<String>,
    pub avatar: Option<VersionedPayload>,
    pub evidence: EvidenceState,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecord {
    pub id: ConversationId,
    pub scope: Scope,
    pub resource: ProviderResourceRef,
    pub kind: ConversationKind,
    pub title: String,
    pub parent_id: Option<ConversationId>,
    pub participant_count: Option<u64>,
    pub participants: Vec<ActorRecord>,
    pub evidence: EvidenceState,
    pub observed_at: DateTime<Utc>,
    pub provider_metadata: Option<VersionedPayload>,
}
impl ConversationRecord {
    pub fn validate(&self, expected: &Scope) -> Result<(), DomainError> {
        validate_record(
            *self.id.as_uuid(),
            &self.scope,
            &self.resource,
            ResourceKind::Conversation,
            expected,
        )?;
        if let Some(metadata) = &self.provider_metadata {
            metadata.validate()?;
        }
        for actor in &self.participants {
            validate_record(
                *actor.id.as_uuid(),
                &actor.scope,
                &actor.resource,
                ResourceKind::Actor,
                expected,
            )?;
            if let Some(avatar) = &actor.avatar {
                avatar.validate()?;
            }
        }
        Ok(())
    }
}

/// Inert display/search metadata only. No remote URL is fetched or interpreted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentRecord {
    pub kind: ContentKind,
    pub safe_display_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub mime_type: Option<String>,
    pub locator: VersionedPayload,
}

/// Transient query data, not a persistent job/store payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentRecord {
    pub id: ContentId,
    pub scope: Scope,
    pub conversation_id: ConversationId,
    pub resource: ProviderResourceRef,
    pub author_id: ActorId,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub kind: ContentKind,
    pub searchable_text: String,
    pub attachments: Vec<AttachmentRecord>,
    pub reply_to: Option<ContentId>,
    pub thread_parent: Option<ConversationId>,
    pub external_location: ExternalLocationAvailability,
    pub evidence: EvidenceState,
    pub observed_at: DateTime<Utc>,
    pub privacy_findings: Vec<PrivacyKind>,
    pub detector_version: Option<String>,
    pub provider_metadata: Option<VersionedPayload>,
}
impl ContentRecord {
    pub fn validate(&self, expected: &Scope) -> Result<(), DomainError> {
        validate_record(
            *self.id.as_uuid(),
            &self.scope,
            &self.resource,
            ResourceKind::Content,
            expected,
        )?;
        if !self.privacy_findings.is_empty()
            && self
                .detector_version
                .as_ref()
                .is_none_or(|v| !crate::identity::bounded_text(v, 128))
        {
            return Err(DomainError::InvalidReference);
        }
        if let Some(metadata) = &self.provider_metadata {
            metadata.validate()?;
        }
        for attachment in &self.attachments {
            attachment.locator.validate()?;
        }
        Ok(())
    }
}

fn validate_record(
    id: uuid::Uuid,
    scope: &Scope,
    resource: &ProviderResourceRef,
    kind: ResourceKind,
    expected: &Scope,
) -> Result<(), DomainError> {
    if resource.resource_kind != kind {
        return Err(DomainError::InvalidReference);
    }
    ScopedResourceRef {
        id,
        scope: scope.clone(),
        resource: resource.clone(),
    }
    .validate(expected)
}
