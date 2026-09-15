use discord_archive::DiscordId;
use retract_domain::*;
use serde::{Deserialize, Serialize};

use super::model::*;
use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
};

pub(crate) const USER_SCHEMA: &str = "discord.user";
pub(crate) const CHANNEL_SCHEMA: &str = "discord.channel";
pub(crate) const MESSAGE_SCHEMA: &str = "discord.message";
pub(crate) const ATTACHMENT_SCHEMA: &str = "discord.attachment";
pub(crate) const VERSION: u16 = 1;

pub(crate) fn discord_provider_key() -> ProviderKey {
    "discord"
        .to_owned()
        .try_into()
        .expect("static provider key")
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscordUserLocator {
    pub user_id: String,
}
impl DiscordUserLocator {
    pub(crate) fn new(user_id: &str) -> Result<Self, AppError> {
        canonical_id(user_id)?;
        Ok(Self {
            user_id: user_id.into(),
        })
    }
    pub(crate) fn canonical_key(&self) -> String {
        self.user_id.clone()
    }
    pub(crate) fn payload(&self) -> VersionedPayload {
        payload(USER_SCHEMA, self).expect("bounded user locator")
    }
    pub(crate) fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Actor,
            USER_SCHEMA,
            self.canonical_key(),
            self,
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscordChannelLocator {
    pub channel_id: String,
}
impl DiscordChannelLocator {
    pub(crate) fn new(channel_id: &str) -> Result<Self, AppError> {
        canonical_id(channel_id)?;
        Ok(Self {
            channel_id: channel_id.into(),
        })
    }
    pub(crate) fn canonical_key(&self) -> String {
        self.channel_id.clone()
    }
    pub(crate) fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Conversation,
            CHANNEL_SCHEMA,
            self.canonical_key(),
            self,
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscordMessageLocator {
    pub channel_id: String,
    pub message_id: String,
}
impl DiscordMessageLocator {
    pub(crate) fn new(channel_id: &str, message_id: &str) -> Result<Self, AppError> {
        canonical_id(channel_id)?;
        canonical_id(message_id)?;
        Ok(Self {
            channel_id: channel_id.into(),
            message_id: message_id.into(),
        })
    }
    pub(crate) fn canonical_key(&self) -> String {
        tuple_key(&[&self.channel_id, &self.message_id])
    }
    pub(crate) fn resource(&self, account_id: AccountId) -> ProviderResourceRef {
        resource(
            account_id,
            ResourceKind::Content,
            MESSAGE_SCHEMA,
            self.canonical_key(),
            self,
        )
    }
}

/// Attachment identity is positional within one message; URL/name are observations.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscordAttachmentLocator {
    pub channel_id: String,
    pub message_id: String,
    pub ordinal: u32,
}

/// One encoding for compound resource keys: ASCII byte length followed by value.
fn tuple_key(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect()
}
fn resource(
    account_id: AccountId,
    resource_kind: ResourceKind,
    schema: &str,
    canonical_key: String,
    value: &impl Serialize,
) -> ProviderResourceRef {
    ProviderResourceRef {
        provider: discord_provider_key(),
        account_id,
        resource_kind,
        locator_schema: schema.into(),
        locator_version: VERSION,
        canonical_key,
        locator_payload: serde_json::to_value(value).expect("typed locator serializes"),
    }
}
pub(super) fn canonical_id(value: &str) -> Result<(), AppError> {
    DiscordId::parse(value).map(|_| ()).map_err(|_| invalid())
}
pub(super) fn invalid() -> AppError {
    AppError::SecureStore("invalid Discord archive payload".into())
}

#[derive(Default)]
pub(crate) struct DiscordPayloadValidator;
impl ProviderPayloadValidator for DiscordPayloadValidator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "discord.archive_payload.v1"
            .to_owned()
            .try_into()
            .expect("static policy key")
    }
    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        account.validate().map_err(|_| invalid())?;
        if account.provider != discord_provider_key()
            || account.connection_state != ConnectionState::Disconnected
            || account.avatar.is_some()
        {
            return Err(invalid());
        }
        display(&account.display_name)?;
        if let Some(username) = &account.username {
            display(username)?;
        }
        let locator: DiscordUserLocator = decode(&account.native_identity, USER_SCHEMA)?;
        canonical_id(&locator.user_id)?;
        locator.canonical_key().try_into()
    }
    fn validate_source(
        &self,
        source: &SourceRecord,
        account: &AccountRecord,
    ) -> Result<(), AppError> {
        self.validate_account(account)?;
        source.validate(account).map_err(|_| invalid())?;
        if source.kind != SourceKind::ArchiveImport {
            return Err(invalid());
        }
        let profile: DiscordSourceProfile = decode(&source.schema_profile, SOURCE_SCHEMA)?;
        if profile.policy_key != IMPORT_POLICY {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        use crate::persistence::archive::{ENVELOPE_BYTES, encoded_size};
        encoded_size(resource, ENVELOPE_BYTES).map_err(|_| invalid())?;
        resource.validate().map_err(|_| invalid())?;
        if resource.provider != discord_provider_key() || resource.locator_version != VERSION {
            return Err(invalid());
        }
        let expected_key = match (resource.resource_kind, resource.locator_schema.as_str()) {
            (ResourceKind::Actor, USER_SCHEMA) => {
                let value: DiscordUserLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid())?;
                canonical_id(&value.user_id)?;
                value.canonical_key()
            }
            (ResourceKind::Conversation, CHANNEL_SCHEMA) => {
                let value: DiscordChannelLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid())?;
                canonical_id(&value.channel_id)?;
                value.canonical_key()
            }
            (ResourceKind::Content, MESSAGE_SCHEMA) => {
                let value: DiscordMessageLocator =
                    serde_json::from_value(resource.locator_payload.clone())
                        .map_err(|_| invalid())?;
                canonical_id(&value.channel_id)?;
                canonical_id(&value.message_id)?;
                value.canonical_key()
            }
            _ => return Err(invalid()),
        };
        if resource.canonical_key != expected_key {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_archive_actor(&self, record: &ActorRecord) -> Result<(), AppError> {
        if record.evidence != EvidenceState::Archive
            || record.resource.resource_kind != ResourceKind::Actor
            || record.avatar.is_some()
        {
            return Err(invalid());
        }
        self.validate_resource(&record.resource)?;
        ScopedResourceRef {
            scope: record.scope.clone(),
            id: *record.id.as_uuid(),
            resource: record.resource.clone(),
        }
        .validate(&record.scope)
        .map_err(|_| invalid())?;
        display(&record.display_name)?;
        if let Some(username) = &record.username {
            display(username)?;
        }
        Ok(())
    }
    fn validate_archive_conversation(&self, record: &ConversationRecord) -> Result<(), AppError> {
        use crate::persistence::archive::{MAX_BATCH_BYTES, MAX_BATCH_RECORDS, encoded_size};
        if record.participants.len() >= MAX_BATCH_RECORDS {
            return Err(invalid());
        }
        encoded_size(record, MAX_BATCH_BYTES).map_err(|_| invalid())?;
        record.validate(&record.scope).map_err(|_| invalid())?;
        self.validate_resource(&record.resource)?;
        if record.evidence != EvidenceState::Archive
            || record.parent_id.is_some()
            || record.participant_count.is_some()
        {
            return Err(invalid());
        }
        display(&record.title)?;
        for actor in &record.participants {
            self.validate_archive_actor(actor)?;
            if actor.observed_at != record.observed_at {
                return Err(invalid());
            }
        }
        let metadata: DiscordConversationMetadata = decode(
            record.provider_metadata.as_ref().ok_or_else(invalid)?,
            CONVERSATION_METADATA_SCHEMA,
        )?;
        metadata.validate()?;
        if record.kind != metadata.verified_kind.normalized()
            || record.title != metadata.channel_name.as_deref().unwrap_or_default()
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn validate_archive_content(&self, record: &ContentRecord) -> Result<(), AppError> {
        use crate::persistence::archive::{
            MAX_ATTACHMENTS, MAX_BATCH_BYTES, MAX_SEARCHABLE_BYTES, encoded_size,
        };
        if record.attachments.len() > MAX_ATTACHMENTS {
            return Err(invalid());
        }
        let search_bytes = record
            .attachments
            .iter()
            .try_fold(record.searchable_text.len(), |sum, attachment| {
                sum.checked_add(attachment.safe_display_name.as_ref().map_or(0, String::len))
            })
            .ok_or_else(invalid)?;
        if search_bytes > MAX_SEARCHABLE_BYTES {
            return Err(invalid());
        }
        encoded_size(record, MAX_BATCH_BYTES).map_err(|_| invalid())?;
        record.validate(&record.scope).map_err(|_| invalid())?;
        self.validate_resource(&record.resource)?;
        if record.evidence != EvidenceState::Archive
            || record.external_location != ExternalLocationAvailability::Unsupported
            || record.edited_at.is_some()
            || record.reply_to.is_some()
            || record.thread_parent.is_some()
        {
            return Err(invalid());
        }
        let message: DiscordMessageLocator =
            serde_json::from_value(record.resource.locator_payload.clone())
                .map_err(|_| invalid())?;
        let metadata: DiscordContentMetadata = decode(
            record.provider_metadata.as_ref().ok_or_else(invalid)?,
            CONTENT_METADATA_SCHEMA,
        )?;
        let actor =
            DiscordUserLocator::new(&metadata.author_user_id)?.resource(record.scope.account_id);
        let channel =
            DiscordChannelLocator::new(&message.channel_id)?.resource(record.scope.account_id);
        if actor.resource_id().map_err(|_| invalid())? != *record.author_id.as_uuid()
            || channel.resource_id().map_err(|_| invalid())? != *record.conversation_id.as_uuid()
        {
            return Err(invalid());
        }
        // Profile v1 establishes at most one opaque URL. No splitting grammar.
        if metadata.attachment_urls.len() > 1
            || record.attachments.len() != metadata.attachment_urls.len()
            || record.kind != content_kind(&record.searchable_text, !record.attachments.is_empty())
        {
            return Err(invalid());
        }
        for (ordinal, (attachment, url)) in record
            .attachments
            .iter()
            .zip(&metadata.attachment_urls)
            .enumerate()
        {
            let locator: DiscordAttachmentLocator = decode(&attachment.locator, ATTACHMENT_SCHEMA)?;
            canonical_id(&locator.channel_id)?;
            canonical_id(&locator.message_id)?;
            if locator.channel_id != message.channel_id
                || locator.message_id != message.message_id
                || locator.ordinal as usize != ordinal
                || attachment.kind != ContentKind::Other
                || attachment.mime_type.is_some()
                || attachment.size_bytes.is_some()
                || attachment.safe_display_name != attachment_name(url)?
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn validate_recipe(&self, _: &RemediationPlan) -> Result<(), AppError> {
        Err(invalid())
    }
    fn validate_job(&self, _: &RemediationPlan, _: &ScopedJobRecord) -> Result<(), AppError> {
        Err(invalid())
    }
}
