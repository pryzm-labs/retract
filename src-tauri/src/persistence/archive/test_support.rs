use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use retract_domain::{
    AccountRecord, ProviderKey, ProviderResourceRef, RemediationPlan, SourceRecord,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
};

use super::{ArchiveKey, ArchiveStore};

pub(in crate::persistence::archive) struct Fixture {
    pub directory: tempfile::TempDir,
    pub path: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(directory.path())
            .unwrap()
            .join("archive.db");
        Self { directory, path }
    }

    pub fn open(&self) -> ArchiveStore {
        ArchiveStore::open(self.path.clone(), key(), validators()).unwrap()
    }
}

pub(in crate::persistence::archive) fn key() -> ArchiveKey {
    ArchiveKey::new([0x84; 32])
}

pub(in crate::persistence::archive) fn provider() -> ProviderKey {
    "synthetic".to_owned().try_into().unwrap()
}

pub(in crate::persistence::archive) fn validators()
-> BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>> {
    BTreeMap::from([(provider(), Arc::new(SyntheticValidator) as Arc<_>)])
}

pub(in crate::persistence::archive) fn account() -> AccountRecord {
    serde_json::from_value(json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "provider": "synthetic",
        "nativeIdentity": {
            "schema": "synthetic.account", "version": 1,
            "payload": {"nativeId": "9007199254740992", "encoding": "decimal"}
        },
        "displayName": "Synthetic archive account", "username": null, "avatar": null,
        "connectionState": "disconnected",
        "createdAt": "2026-09-05T00:00:00Z", "lastSeenAt": "2026-09-05T00:00:00Z"
    }))
    .unwrap()
}

pub(in crate::persistence::archive) fn source() -> SourceRecord {
    serde_json::from_value(json!({
        "id": "22222222-2222-4222-8222-222222222222",
        "accountId": "11111111-1111-4111-8111-111111111111",
        "provider": "synthetic", "kind": "archive_import", "state": "preparing",
        "archiveFingerprint": "synthetic-export-01",
        "schemaProfile": {"schema": "synthetic.archive", "version": 1, "payload": {"format": "fixture"}},
        "importedAt": null, "updatedAt": "2026-09-05T00:00:00Z", "warnings": []
    })).unwrap()
}

pub(in crate::persistence::archive) fn resource(native_id: &str) -> ProviderResourceRef {
    serde_json::from_value(json!({
        "provider": "synthetic", "accountId": "11111111-1111-4111-8111-111111111111",
        "resourceKind": "content", "locatorSchema": "synthetic.content", "locatorVersion": 1,
        "canonicalKey": native_id, "locatorPayload": {"nativeId": native_id}
    }))
    .unwrap()
}

struct SyntheticValidator;

pub(in crate::persistence::archive) fn duplicate_lock_description(
    store: &ArchiveStore,
) -> fs::File {
    store._process_lock.0.try_clone().unwrap()
}

fn invalid() -> AppError {
    AppError::SecureStore("invalid synthetic payload".into())
}

impl ProviderPayloadValidator for SyntheticValidator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "synthetic-archive-v1".to_owned().try_into().unwrap()
    }

    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Identity {
            native_id: String,
            encoding: String,
        }
        if account.provider != provider()
            || account.native_identity.schema != "synthetic.account"
            || account.native_identity.version != 1
            || account.avatar.is_some()
        {
            return Err(invalid());
        }
        let parsed: Identity = serde_json::from_value(account.native_identity.payload.clone())
            .map_err(|_| invalid())?;
        let native = match parsed.encoding.as_str() {
            "decimal" => parsed.native_id.parse::<u64>(),
            "hex" => u64::from_str_radix(&parsed.native_id, 16),
            _ => return Err(invalid()),
        }
        .map_err(|_| invalid())?;
        if native == 0 {
            return Err(invalid());
        }
        format!("synthetic:{native}").try_into()
    }

    fn validate_source(&self, source: &SourceRecord, _: &AccountRecord) -> Result<(), AppError> {
        if source.schema_profile.schema != "synthetic.archive"
            || source.schema_profile.version != 1
            || source.schema_profile.payload != json!({"format": "fixture"})
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        if resource.provider != provider()
            || resource.locator_schema != "synthetic.content"
            || resource.locator_version != 1
            || resource.locator_payload != json!({"nativeId": resource.canonical_key})
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_recipe(&self, _: &RemediationPlan) -> Result<(), AppError> {
        Err(invalid())
    }
}
