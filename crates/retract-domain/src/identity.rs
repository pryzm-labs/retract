use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::DomainError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderKey(String);

impl TryFrom<String> for ProviderKey {
    type Error = DomainError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !(1..=64).contains(&value.len())
            || !value.as_bytes()[0].is_ascii_lowercase()
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(DomainError::InvalidProviderKey);
        }
        Ok(Self(value))
    }
}
impl From<ProviderKey> for String {
    fn from(value: ProviderKey) -> Self {
        value.0
    }
}
impl ProviderKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);
        impl $name {
            pub fn as_uuid(&self) -> &Uuid { &self.0 }
        }
        impl TryFrom<Uuid> for $name {
            type Error = DomainError;
            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                if value.is_nil() { return Err(DomainError::InvalidIdentity); }
                Ok(Self(value))
            }
        }
        impl From<$name> for Uuid {
            fn from(value: $name) -> Self { value.0 }
        }
    )+};
}
identity!(
    AccountId,
    SourceId,
    ConversationId,
    ContentId,
    ActorId,
    GroupingId
);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub provider: ProviderKey,
    pub account_id: AccountId,
    pub source_id: SourceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveContext {
    pub scope: Scope,
    pub session_generation: Uuid,
}
impl ActiveContext {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.session_generation.is_nil() {
            return Err(DomainError::InvalidIdentity);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Conversation,
    Content,
    Actor,
    Grouping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderResourceRef {
    pub provider: ProviderKey,
    pub account_id: AccountId,
    pub resource_kind: ResourceKind,
    pub locator_schema: String,
    pub locator_version: u16,
    pub canonical_key: String,
    pub locator_payload: Value,
}
impl ProviderResourceRef {
    /// Envelope validation only. The owning provider must additionally validate
    /// schema support, typed payload, canonical key and native ranges.
    pub fn validate(&self) -> Result<(), DomainError> {
        if !bounded_text(&self.locator_schema, 128)
            || self.locator_version == 0
            || !bounded_text(&self.canonical_key, 4096)
        {
            return Err(DomainError::InvalidReference);
        }
        Ok(())
    }
    pub fn resource_id(&self) -> Result<Uuid, DomainError> {
        self.validate()?;
        let name = serde_json::to_vec(&(
            "retract-resource-v1",
            self.resource_kind,
            &self.locator_schema,
            self.locator_version,
            &self.canonical_key,
        ))
        .map_err(|_| DomainError::InvalidReference)?;
        Ok(Uuid::new_v5(self.account_id.as_uuid(), &name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedResourceRef {
    pub scope: Scope,
    pub id: Uuid,
    pub resource: ProviderResourceRef,
}
impl ScopedResourceRef {
    /// `expected` must come from a verified source/account relationship, not the
    /// untrusted request itself. Source ownership is checked by the store/port.
    pub fn validate(&self, expected: &Scope) -> Result<(), DomainError> {
        if &self.scope != expected
            || self.resource.provider != expected.provider
            || self.resource.account_id != expected.account_id
        {
            return Err(DomainError::ScopeMismatch);
        }
        if self.id != self.resource.resource_id()? {
            return Err(DomainError::InvalidReference);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionedPayload {
    pub schema: String,
    pub version: u16,
    pub payload: Value,
}
impl VersionedPayload {
    pub fn validate(&self) -> Result<(), DomainError> {
        if !bounded_text(&self.schema, 128) || self.version == 0 {
            return Err(DomainError::InvalidReference);
        }
        Ok(())
    }
}

pub(crate) fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
