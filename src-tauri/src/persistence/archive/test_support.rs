use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use retract_domain::{
    AccountRecord, ActorRecord, ContentRecord, ConversationRecord, ProviderKey,
    ProviderResourceRef, RemediationPlan, ResourceKind, SourceRecord,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    error::AppError,
    persistence::{
        ProviderPayloadValidator, ProviderValidationPolicyKey, VerifiedNativeAccountIdentity,
    },
};

use super::super::model::ImportBatch;
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

pub(in crate::persistence::archive) fn account_dependent_validators()
-> BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>> {
    BTreeMap::from([(provider(), Arc::new(AccountDependentValidator) as Arc<_>)])
}

struct AccountDependentValidator;

impl ProviderPayloadValidator for AccountDependentValidator {
    fn validation_policy_key(&self) -> ProviderValidationPolicyKey {
        "synthetic-account-dependent-v1"
            .to_owned()
            .try_into()
            .unwrap()
    }

    fn validate_account(
        &self,
        account: &AccountRecord,
    ) -> Result<VerifiedNativeAccountIdentity, AppError> {
        SyntheticValidator.validate_account(account)
    }

    fn validate_source(
        &self,
        source: &SourceRecord,
        account: &AccountRecord,
    ) -> Result<(), AppError> {
        if source.schema_profile.schema != "synthetic.archive"
            || source.schema_profile.version != 1
            || source.schema_profile.payload != json!({"accountName": account.display_name})
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_resource(&self, resource: &ProviderResourceRef) -> Result<(), AppError> {
        SyntheticValidator.validate_resource(resource)
    }

    fn validate_recipe(&self, plan: &RemediationPlan) -> Result<(), AppError> {
        SyntheticValidator.validate_recipe(plan)
    }
}

pub(in crate::persistence::archive) fn snapshot_recovery_files(
    path: &std::path::Path,
) -> Vec<(String, Option<Vec<u8>>)> {
    ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let bytes = match fs::read(PathBuf::from(name)) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("could not snapshot synthetic recovery artifact: {error}"),
            };
            (suffix.to_owned(), bytes)
        })
        .collect()
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
        "synthetic-archive-v2".to_owned().try_into().unwrap()
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

    fn validate_archive_actor(&self, record: &retract_domain::ActorRecord) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        validate_metadata(record.avatar.as_ref())
    }

    fn validate_archive_conversation(
        &self,
        record: &retract_domain::ConversationRecord,
    ) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        validate_metadata(record.provider_metadata.as_ref())?;
        for actor in &record.participants {
            self.validate_archive_actor(actor)?;
        }
        Ok(())
    }

    fn validate_archive_content(
        &self,
        record: &retract_domain::ContentRecord,
    ) -> Result<(), AppError> {
        self.validate_resource(&record.resource)?;
        validate_metadata(record.provider_metadata.as_ref())?;
        for attachment in &record.attachments {
            validate_metadata(Some(&attachment.locator))?;
        }
        Ok(())
    }
}

fn validate_metadata(envelope: Option<&retract_domain::VersionedPayload>) -> Result<(), AppError> {
    if let Some(envelope) = envelope {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Metadata {
            note: String,
        }
        if envelope.schema != "synthetic.metadata" || envelope.version != 1 {
            return Err(invalid());
        }
        let metadata: Metadata =
            serde_json::from_value(envelope.payload.clone()).map_err(|_| invalid())?;
        let _ = metadata.note;
    }
    Ok(())
}

pub(in crate::persistence::archive) fn batch(native: &str, text: &str) -> ImportBatch {
    let scope = source().scope();
    let mut actor_ref = resource("author");
    actor_ref.resource_kind = ResourceKind::Actor;
    let mut conversation_ref = resource("room");
    conversation_ref.resource_kind = ResourceKind::Conversation;
    let actor: ActorRecord = serde_json::from_value(json!({
        "id": actor_ref.resource_id().unwrap(), "scope": scope, "resource": actor_ref,
        "displayName": "Synthetic actor", "username": null, "avatar": null,
        "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z"
    }))
    .unwrap();
    let conversation: ConversationRecord = serde_json::from_value(json!({
        "id": conversation_ref.resource_id().unwrap(), "scope": scope, "resource": conversation_ref,
        "kind": "group", "title": "Synthetic room", "parentId": null, "participantCount": 1,
        "participants": [], "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z", "providerMetadata": null
    })).unwrap();
    let content_ref = resource(native);
    let content: ContentRecord = serde_json::from_value(json!({
        "id": content_ref.resource_id().unwrap(), "scope": scope, "resource": content_ref,
        "conversationId": conversation.id, "authorId": actor.id,
        "timestamp": "2026-09-05T00:00:00Z", "editedAt": null, "kind": "text",
        "searchableText": text, "attachments": [], "replyTo": null, "threadParent": null,
        "externalLocation": "unavailable", "evidence": "archive", "observedAt": "2026-09-05T00:00:00Z",
        "privacyFindings": [], "detectorVersion": null, "providerMetadata": null
    })).unwrap();
    ImportBatch {
        actors: vec![actor],
        conversations: vec![conversation],
        contents: vec![content],
    }
}

pub(in crate::persistence::archive) fn registered(fixture: &Fixture) -> ArchiveStore {
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    store
}

pub(in crate::persistence::archive) fn stored_texts(store: &ArchiveStore) -> Vec<String> {
    store
        .transaction(|tx| {
            Ok(tx
                .prepare("SELECT searchable_text FROM content_observations ORDER BY resource_id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap())
        })
        .unwrap()
}

pub(in crate::persistence::archive) fn checkpoint(
    store: &ArchiveStore,
) -> super::super::model::ImportCheckpoint {
    store
        .import_status(
            &source().scope(),
            source().archive_fingerprint.as_deref().unwrap(),
            &source().schema_profile,
        )
        .unwrap()
        .unwrap()
}

pub(in crate::persistence::archive) fn sql_snapshot(
    store: &ArchiveStore,
) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    store
        .transaction(|tx| {
            Ok([
                "resource_identities",
                "conversation_observations",
                "actor_observations",
                "content_observations",
                "attachments",
                "privacy_findings",
                "import_runs",
                "import_batch_receipts",
                "import_warnings",
            ]
            .into_iter()
            .map(|table| {
                let mut query = tx
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let columns = query.column_count();
                query
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| row.get(column))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            })
            .collect())
        })
        .unwrap()
}

pub(in crate::persistence::archive) fn fts_count(store: &ArchiveStore, query: &str) -> i64 {
    store
        .transaction(|tx| {
            Ok(tx
                .query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH ?",
                    [query],
                    |row| row.get(0),
                )
                .unwrap())
        })
        .unwrap()
}

pub(in crate::persistence::archive) fn envelope(note: String) -> retract_domain::VersionedPayload {
    serde_json::from_value(
        json!({"schema": "synthetic.metadata", "version": 1, "payload": {"note": note}}),
    )
    .unwrap()
}

pub(in crate::persistence::archive) fn attachment(name: &str) -> retract_domain::AttachmentRecord {
    serde_json::from_value(json!({"kind": "document", "safeDisplayName": name, "sizeBytes": 5, "mimeType": "application/octet-stream", "locator": envelope("inert".into())})).unwrap()
}
