use super::{locators::*, model::*};
use crate::{error::AppError, persistence::ProviderPayloadValidator};
use chrono::{DateTime, Utc};
use discord_archive::{ChannelContext, ExportAccount, SentMessage};
use retract_domain::*;

pub(crate) struct DiscordNormalizer {
    scope: Scope,
    observed_at: DateTime<Utc>,
}
impl DiscordNormalizer {
    pub(crate) fn new(scope: Scope, observed_at: DateTime<Utc>) -> Result<Self, AppError> {
        if scope.provider != discord_provider_key() {
            return Err(invalid());
        }
        Ok(Self { scope, observed_at })
    }
    pub(crate) fn account(&self, input: &ExportAccount) -> Result<AccountRecord, AppError> {
        display(&input.username)?;
        let record = AccountRecord {
            id: self.scope.account_id,
            provider: discord_provider_key(),
            native_identity: DiscordUserLocator::new(input.id.as_str())?.payload(),
            display_name: input.username.clone(),
            username: Some(input.username.clone()),
            avatar: None,
            connection_state: ConnectionState::Disconnected,
            created_at: self.observed_at,
            last_seen_at: self.observed_at,
        };
        DiscordPayloadValidator.validate_account(&record)?;
        Ok(record)
    }
    pub(crate) fn actor(&self, input: &ExportAccount) -> Result<ActorRecord, AppError> {
        display(&input.username)?;
        let resource = DiscordUserLocator::new(input.id.as_str())?.resource(self.scope.account_id);
        let record = ActorRecord {
            id: resource
                .resource_id()
                .map_err(|_| invalid())?
                .try_into()
                .map_err(|_| invalid())?,
            scope: self.scope.clone(),
            resource,
            display_name: input.username.clone(),
            username: Some(input.username.clone()),
            avatar: None,
            evidence: EvidenceState::Archive,
            observed_at: self.observed_at,
        };
        DiscordPayloadValidator.validate_archive_actor(&record)?;
        Ok(record)
    }
    /// The current profile proves no semantic channel discriminator.
    pub(crate) fn conversation(
        &self,
        input: &ChannelContext,
    ) -> Result<ConversationRecord, AppError> {
        self.conversation_with_kind(input, VerifiedConversationKind::Other)
    }
    pub(crate) fn conversation_with_kind(
        &self,
        input: &ChannelContext,
        kind: VerifiedConversationKind,
    ) -> Result<ConversationRecord, AppError> {
        display(&input.source_type)?;
        if let Some(name) = &input.name {
            display(name)?;
        }
        if input.guild.is_some() && (input.name.is_none() || input.recipients.is_some()) {
            return Err(invalid());
        }
        // Bound borrowed source metadata before cloning potentially large recipients.
        let raw_metadata = (&input.source_type, &input.name, &input.recipients);
        crate::persistence::archive::encoded_size(
            &raw_metadata,
            crate::persistence::archive::ENVELOPE_BYTES,
        )
        .map_err(|_| invalid())?;
        let metadata = DiscordConversationMetadata {
            source_type: input.source_type.clone(),
            channel_name: input.name.clone(),
            verified_kind: kind,
            guild: input.guild.as_ref().map(|guild| DiscordGuildMetadata {
                id: guild.id.as_str().into(),
                name: guild.name.clone(),
            }),
            recipients: input.recipients.clone(),
            warnings: if kind == VerifiedConversationKind::Other {
                vec![DiscordWarning::UnknownConversationKind]
            } else {
                vec![]
            },
        };
        metadata.validate()?;
        let resource =
            DiscordChannelLocator::new(input.id.as_str())?.resource(self.scope.account_id);
        let record = ConversationRecord {
            id: resource
                .resource_id()
                .map_err(|_| invalid())?
                .try_into()
                .map_err(|_| invalid())?,
            scope: self.scope.clone(),
            resource,
            kind: kind.normalized(),
            title: input.name.clone().unwrap_or_default(),
            parent_id: None,
            participant_count: None,
            participants: vec![],
            evidence: EvidenceState::Archive,
            observed_at: self.observed_at,
            provider_metadata: Some(payload(CONVERSATION_METADATA_SCHEMA, &metadata)?),
        };
        DiscordPayloadValidator.validate_archive_conversation(&record)?;
        Ok(record)
    }
    pub(crate) fn content(
        &self,
        account: &ExportAccount,
        channel: &ChannelContext,
        input: &SentMessage,
    ) -> Result<ContentRecord, AppError> {
        if input.account_id != account.id
            || input.channel_id != channel.id
            || input.contents.len() > crate::persistence::archive::MAX_SEARCHABLE_BYTES
        {
            return Err(invalid());
        }
        let timestamp =
            DateTime::from_timestamp_millis(input.timestamp_millis).ok_or_else(invalid)?;
        let resource = DiscordMessageLocator::new(input.channel_id.as_str(), input.id.as_str())?
            .resource(self.scope.account_id);
        let actor = DiscordUserLocator::new(account.id.as_str())?.resource(self.scope.account_id);
        let conversation =
            DiscordChannelLocator::new(channel.id.as_str())?.resource(self.scope.account_id);
        let attachments = if input.attachments.is_empty() {
            vec![]
        } else {
            vec![AttachmentRecord {
                kind: ContentKind::Other,
                safe_display_name: attachment_name(&input.attachments)?,
                size_bytes: None,
                mime_type: None,
                locator: payload(
                    ATTACHMENT_SCHEMA,
                    &DiscordAttachmentLocator {
                        channel_id: input.channel_id.as_str().into(),
                        message_id: input.id.as_str().into(),
                        ordinal: 0,
                    },
                )?,
            }]
        };
        let metadata = DiscordContentMetadata {
            author_user_id: account.id.as_str().into(),
            attachment_urls: if input.attachments.is_empty() {
                vec![]
            } else {
                vec![input.attachments.clone()]
            },
        };
        let record = ContentRecord {
            id: resource
                .resource_id()
                .map_err(|_| invalid())?
                .try_into()
                .map_err(|_| invalid())?,
            scope: self.scope.clone(),
            conversation_id: conversation
                .resource_id()
                .map_err(|_| invalid())?
                .try_into()
                .map_err(|_| invalid())?,
            resource,
            author_id: actor
                .resource_id()
                .map_err(|_| invalid())?
                .try_into()
                .map_err(|_| invalid())?,
            timestamp,
            edited_at: None,
            kind: content_kind(&input.contents, !attachments.is_empty()),
            searchable_text: input.contents.clone(),
            attachments,
            reply_to: None,
            thread_parent: None,
            external_location: ExternalLocationAvailability::Unsupported,
            evidence: EvidenceState::Archive,
            observed_at: self.observed_at,
            privacy_findings: vec![],
            detector_version: None,
            provider_metadata: Some(payload(CONTENT_METADATA_SCHEMA, &metadata)?),
        };
        DiscordPayloadValidator.validate_archive_content(&record)?;
        Ok(record)
    }
}
