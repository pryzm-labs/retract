use super::locators::{VERSION, canonical_id, invalid};
use crate::{
    error::AppError,
    persistence::archive::{ENVELOPE_BYTES, encoded_size},
};
use retract_domain::{ContentKind, ConversationKind, VersionedPayload};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) const SOURCE_SCHEMA: &str = "discord.data_package.messages_json";
pub(crate) const IMPORT_POLICY: &str = "discord.import_policy.v1";
pub(super) const CONVERSATION_METADATA_SCHEMA: &str = "discord.conversation_metadata";
pub(super) const CONTENT_METADATA_SCHEMA: &str = "discord.content_metadata";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerifiedConversationKind {
    Direct,
    GroupDirect,
    GuildChannel,
    Other,
}
impl VerifiedConversationKind {
    pub(super) fn normalized(self) -> ConversationKind {
        match self {
            Self::Direct => ConversationKind::Direct,
            Self::GroupDirect => ConversationKind::Group,
            Self::GuildChannel => ConversationKind::CommunityChannel,
            Self::Other => ConversationKind::Other,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscordSourceProfile {
    pub policy_key: String,
}
impl DiscordSourceProfile {
    pub(crate) fn payload() -> VersionedPayload {
        payload(
            SOURCE_SCHEMA,
            &Self {
                policy_key: IMPORT_POLICY.into(),
            },
        )
        .expect("static source profile")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiscordGuildMetadata {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiscordConversationMetadata {
    pub source_type: String,
    pub channel_name: Option<String>,
    pub verified_kind: VerifiedConversationKind,
    pub guild: Option<DiscordGuildMetadata>,
    pub recipients: Option<Vec<String>>,
    pub warnings: Vec<DiscordWarning>,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DiscordWarning {
    UnknownConversationKind,
}
impl DiscordConversationMetadata {
    pub(super) fn validate(&self) -> Result<(), AppError> {
        display(&self.source_type)?;
        if let Some(name) = &self.channel_name {
            display(name)?;
        }
        if let Some(guild) = &self.guild {
            canonical_id(&guild.id)?;
            display(&guild.name)?;
            if self.recipients.is_some() || self.channel_name.is_none() {
                return Err(invalid());
            }
        }
        if let Some(recipients) = &self.recipients {
            for recipient in recipients {
                display(recipient)?;
            }
        }
        let warnings = if self.verified_kind == VerifiedConversationKind::Other {
            vec![DiscordWarning::UnknownConversationKind]
        } else {
            vec![]
        };
        if self.warnings != warnings {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiscordContentMetadata {
    pub author_user_id: String,
    pub attachment_urls: Vec<String>,
}

pub(super) fn payload(schema: &str, value: &impl Serialize) -> Result<VersionedPayload, AppError> {
    // Bound the complete envelope before constructing a JSON value.
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        schema: &'a str,
        version: u16,
        payload: &'a T,
    }
    encoded_size(
        &Envelope {
            schema,
            version: VERSION,
            payload: value,
        },
        ENVELOPE_BYTES,
    )
    .map_err(|_| invalid())?;
    Ok(VersionedPayload {
        schema: schema.into(),
        version: VERSION,
        payload: serde_json::to_value(value).map_err(|_| invalid())?,
    })
}
pub(super) fn decode<T: DeserializeOwned>(
    value: &VersionedPayload,
    schema: &str,
) -> Result<T, AppError> {
    encoded_size(value, ENVELOPE_BYTES).map_err(|_| invalid())?;
    if value.schema != schema || value.version != VERSION {
        return Err(invalid());
    }
    serde_json::from_value(value.payload.clone()).map_err(|_| invalid())
}
pub(super) fn display(value: &str) -> Result<(), AppError> {
    if value.len() as u64 > discord_archive::ArchiveLimits::default().max_display_bytes
        || value.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn content_kind(text: &str, has_attachment: bool) -> ContentKind {
    if text.is_empty() && has_attachment {
        ContentKind::Other
    } else {
        ContentKind::Text
    }
}

pub(super) fn attachment_urls(value: &str) -> Result<Vec<&str>, AppError> {
    if value.is_empty() {
        return Ok(vec![]);
    }
    if value.chars().any(|ch| ch.is_whitespace() && ch != ' ') {
        return Err(invalid());
    }
    let urls = value.split(' ').collect::<Vec<_>>();
    if urls.iter().any(|url| url.is_empty())
        || urls.len() > crate::persistence::archive::MAX_ATTACHMENTS
    {
        return Err(invalid());
    }
    for url in &urls {
        attachment_name(url)?;
    }
    Ok(urls)
}

/// Parse inert syntax only. Preserve the caller's exact URL in observation metadata.
pub(super) fn attachment_name(value: &str) -> Result<Option<String>, AppError> {
    if value.len() > ENVELOPE_BYTES
        || !value.starts_with("https://")
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || ch == '\\')
        || value[8..].contains("://")
    {
        return Err(invalid());
    }
    // WHATWG parsing repairs extra slashes; this profile accepts an actual authority.
    if value[8..].starts_with('/') {
        return Err(invalid());
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(invalid());
        }
    }
    let url = url::Url::parse(value).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    let last = url
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .unwrap_or("");
    let name = percent_encoding::percent_decode_str(last)
        .decode_utf8()
        .map_err(|_| invalid())?;
    display(&name)?;
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(invalid());
    }
    Ok((!name.is_empty()).then(|| name.into_owned()))
}
