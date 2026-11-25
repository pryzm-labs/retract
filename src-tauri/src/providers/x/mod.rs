//! X archive observations. No live access or remediation ports.
mod application;
pub(crate) mod commands;
mod import;
mod parser;
#[cfg(test)]
mod tests;
pub(crate) use application::build_connection;
pub(crate) use import::import_file;

use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
};
use retract_domain::*;
use serde_json::json;
pub(crate) fn key() -> ProviderKey {
    "x".to_owned().try_into().expect("static provider key")
}
fn invalid() -> AppError {
    AppError::SecureStore("invalid X archive record".into())
}
fn require(valid: bool) -> Result<(), AppError> {
    if valid { Ok(()) } else { Err(invalid()) }
}
fn profile(multipart: bool) -> VersionedPayload {
    VersionedPayload {
        schema: if multipart {
            "x.ytd.multipart"
        } else {
            "x.ytd.single_part"
        }
        .into(),
        version: 1,
        payload: json!({}),
    }
}
fn user(id: &str) -> VersionedPayload {
    VersionedPayload {
        schema: "x.user".into(),
        version: 1,
        payload: json!({"id":id}),
    }
}
fn resource(scope: &Scope, kind: ResourceKind, id: &str) -> ProviderResourceRef {
    let schema = match kind {
        ResourceKind::Actor => "x.user",
        ResourceKind::Conversation => "x.feed",
        ResourceKind::Content => "x.post",
        _ => unreachable!(),
    };
    ProviderResourceRef {
        provider: key(),
        account_id: scope.account_id,
        resource_kind: kind,
        locator_schema: schema.into(),
        locator_version: 1,
        canonical_key: id.into(),
        locator_payload: json!({"id":id}),
    }
}
pub(crate) struct Validator;
impl ProviderPayloadValidator for Validator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "x.archive.v1".to_owned().try_into().expect("static policy")
    }
    fn validate_account(
        &self,
        a: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        a.validate().map_err(|_| invalid())?;
        let id = a.native_identity.payload["id"]
            .as_str()
            .ok_or_else(invalid)?;
        require(
            a.provider == key()
                && parser::valid_id(id)
                && a.native_identity == user(id)
                && parser::valid_name(&a.display_name)
                && a.username.as_ref() == Some(&a.display_name)
                && a.avatar.is_none()
                && a.connection_state == ConnectionState::Disconnected,
        )?;
        id.to_owned().try_into()
    }
    fn validate_source(&self, s: &SourceRecord, a: &AccountRecord) -> Result<(), AppError> {
        self.validate_account(a)?;
        s.validate(a).map_err(|_| invalid())?;
        require(
            s.kind == SourceKind::ArchiveImport
                && (s.schema_profile == profile(false) || s.schema_profile == profile(true)),
        )
    }
    fn validate_resource(&self, r: &ProviderResourceRef) -> Result<(), AppError> {
        r.validate().map_err(|_| invalid())?;
        let schema = match r.resource_kind {
            ResourceKind::Actor => "x.user",
            ResourceKind::Content => "x.post",
            ResourceKind::Conversation => "x.feed",
            _ => return Err(invalid()),
        };
        require(
            r.provider == key()
                && r.locator_schema == schema
                && r.locator_version == 1
                && parser::valid_id(&r.canonical_key)
                && r.locator_payload == json!({"id":r.canonical_key}),
        )
    }
    fn validate_archive_conversation(&self, r: &ConversationRecord) -> Result<(), AppError> {
        r.validate(&r.scope).map_err(|_| invalid())?;
        self.validate_resource(&r.resource)?;
        require(
            r.kind == ConversationKind::AccountFeed
                && matches!(r.title.as_str(), "Archived posts" | "Authored posts")
                && r.parent_id.is_none()
                && r.participants.is_empty()
                && r.participant_count.is_none()
                && r.evidence == EvidenceState::Archive
                && r.provider_metadata.is_none(),
        )
    }
    fn validate_archive_actor(&self, r: &ActorRecord) -> Result<(), AppError> {
        self.validate_resource(&r.resource)?;
        require(
            r.resource.resource_kind == ResourceKind::Actor
                && r.scope.provider == key()
                && r.scope.account_id == r.resource.account_id
                && r.resource.resource_id().map_err(|_| invalid())? == *r.id.as_uuid()
                && r.evidence == EvidenceState::Archive
                && r.avatar.is_none()
                && parser::valid_name(&r.display_name)
                && r.username.as_ref() == Some(&r.display_name),
        )
    }
    fn validate_archive_content(&self, r: &ContentRecord) -> Result<(), AppError> {
        r.validate(&r.scope).map_err(|_| invalid())?;
        self.validate_resource(&r.resource)?;
        require(
            r.kind == ContentKind::Text
                && r.searchable_text.len() <= 64 * 1024
                && r.evidence == EvidenceState::Archive
                && r.edited_at.is_none()
                && r.thread_parent.is_none()
                && r.reply_to.is_none()
                && r.attachments.is_empty()
                && r.provider_metadata.is_none()
                && r.external_location == ExternalLocationAvailability::Unsupported,
        )
    }
    fn validate_recipe(&self, _: &RemediationPlan) -> Result<(), AppError> {
        Err(invalid())
    }
}
