//! Read-only semantic checks over copied archive observations and projections.

use std::{collections::BTreeMap, sync::Arc};

use retract_domain::{
    ActorRecord, ContentRecord, ConversationRecord, ProviderKey, ProviderResourceRef, ResourceKind,
    Scope,
};
use rusqlite::{Connection, params, types::Value};

use crate::persistence::ProviderPayloadValidator;

use super::{ArchiveError, ingest_state::storage, model};

pub(super) fn validate(
    connection: &Connection,
    validators: &BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
) -> Result<(), ArchiveError> {
    validate_catalog(connection, validators)?;
    let mut ready_sources = connection.prepare("SELECT s.record_json FROM sources s JOIN import_runs r USING(provider, account_id, source_id) WHERE r.state='ready'").map_err(storage)?;
    let mut sources = ready_sources.query([]).map_err(storage)?;
    while let Some(row) = sources.next().map_err(storage)? {
        let source: retract_domain::SourceRecord =
            model::decode(&row.get::<_, String>(0).map_err(storage)?)?;
        if super::ingest::missing_mandatory_observations(connection, &source.scope())? {
            return Err(ArchiveError::InvalidStore);
        }
    }
    for table in [
        "actor_observations",
        "conversation_observations",
        "content_observations",
    ] {
        let mut query = connection.prepare(&format!("SELECT provider, account_id, source_id, resource_id, record_json, resource_kind FROM {table}")).map_err(storage)?;
        let mut rows = query.query([]).map_err(storage)?;
        while let Some(row) = rows.next().map_err(storage)? {
            let json: String = row.get(4).map_err(storage)?;
            let raw: serde_json::Value = model::decode(&json)?;
            let mut batch = model::ImportBatch::default();
            let (scope, id, resource) = match table {
                "actor_observations" => {
                    let record: ActorRecord = model::decode(&json)?;
                    let out = (
                        record.scope.clone(),
                        *record.id.as_uuid(),
                        record.resource.clone(),
                    );
                    batch.actors.push(record);
                    out
                }
                "conversation_observations" => {
                    let record: ConversationRecord = model::decode(&json)?;
                    validate_uuid_text(
                        &raw,
                        "/parentId",
                        record.parent_id.map(|id| *id.as_uuid()),
                    )?;
                    if let Some(parent) = record.parent_id {
                        // This is the ingestion catalog policy, not a same-source
                        // observation requirement. Optional parents may survive
                        // another snapshot's removal or be absent altogether.
                        super::ingest::check_reference(
                            connection,
                            &record.scope,
                            parent.as_uuid(),
                            ResourceKind::Conversation,
                        )?;
                    }
                    let out = (
                        record.scope.clone(),
                        *record.id.as_uuid(),
                        record.resource.clone(),
                    );
                    for (index, actor) in record.participants.iter().enumerate() {
                        let participant = raw
                            .get("participants")
                            .and_then(|value| value.get(index))
                            .ok_or(ArchiveError::InvalidStore)?;
                        validate_observation_uuid_text(
                            participant,
                            &actor.scope,
                            *actor.id.as_uuid(),
                            &actor.resource,
                        )?;
                        validate_catalog_reference(connection, &actor.resource)?;
                    }
                    batch.conversations.push(record);
                    out
                }
                _ => {
                    let mut record: ContentRecord = model::decode(&json)?;
                    for (pointer, id) in [
                        ("/conversationId", Some(*record.conversation_id.as_uuid())),
                        ("/authorId", Some(*record.author_id.as_uuid())),
                        ("/replyTo", record.reply_to.map(|id| *id.as_uuid())),
                        (
                            "/threadParent",
                            record.thread_parent.map(|id| *id.as_uuid()),
                        ),
                    ] {
                        validate_uuid_text(&raw, pointer, id)?;
                    }
                    // This is a validation view. Historical store output is
                    // checked separately and the copied record JSON is untouched.
                    record
                        .validate(&record.scope)
                        .map_err(|_| ArchiveError::InvalidRecord)?;
                    // ContentRecord's finding/version relationship only checks
                    // this bound when findings exist. Persisted detector output
                    // must retain the same bounded-text contract even when the
                    // historical detector found nothing.
                    if record.detector_version.as_ref().is_some_and(|version| {
                        version.trim().is_empty()
                            || version.len() > 128
                            || version.chars().any(char::is_control)
                    }) {
                        return Err(ArchiveError::InvalidStore);
                    }
                    validate_content_projections(connection, &record)?;
                    let out = (
                        record.scope.clone(),
                        *record.id.as_uuid(),
                        record.resource.clone(),
                    );
                    model::clear_derived_fields(&mut record);
                    batch.contents.push(record);
                    out
                }
            };
            validate_observation_uuid_text(&raw, &scope, id, &resource)?;
            if row.get::<_, String>(0).map_err(storage)? != scope.provider.as_str()
                || row.get::<_, String>(1).map_err(storage)?
                    != scope.account_id.as_uuid().to_string()
                || row.get::<_, String>(2).map_err(storage)?
                    != scope.source_id.as_uuid().to_string()
                || row.get::<_, String>(3).map_err(storage)? != id.to_string()
                || row.get::<_, String>(5).map_err(storage)? != kind(&resource.resource_kind)?
            {
                return Err(ArchiveError::InvalidStore);
            }
            validate_catalog_reference(connection, &resource)?;
            let validator = validators
                .get(&scope.provider)
                .ok_or(ArchiveError::InvalidRecord)?;
            // All provider-input limits apply, but persisted detector output
            // cannot consume that budget or be recomputed during migration.
            batch.bounded_size()?;
            batch.validate(&scope, validator.as_ref())?;
        }
    }
    Ok(())
}

// UUID deserialization accepts URNs, braces and other noncanonical spellings.
// Persisted JSON is also used as a SQL projection (notably parentId), so typed
// equality alone can hide text that bypasses catalog joins. Compare only UUID
// fields: historical timestamps, provider payloads and detector output stay
// byte-for-byte untouched and need not be reserialized canonically.
fn validate_uuid_text(
    raw: &serde_json::Value,
    pointer: &str,
    expected: Option<uuid::Uuid>,
) -> Result<(), ArchiveError> {
    let actual = raw.pointer(pointer);
    match expected {
        Some(id) if actual.and_then(serde_json::Value::as_str) == Some(id.to_string().as_str()) => {
            Ok(())
        }
        None if actual.is_none_or(serde_json::Value::is_null) => Ok(()),
        _ => Err(ArchiveError::InvalidStore),
    }
}

fn validate_observation_uuid_text(
    raw: &serde_json::Value,
    scope: &Scope,
    id: uuid::Uuid,
    resource: &ProviderResourceRef,
) -> Result<(), ArchiveError> {
    for (pointer, id) in [
        ("/id", id),
        ("/scope/accountId", *scope.account_id.as_uuid()),
        ("/scope/sourceId", *scope.source_id.as_uuid()),
        ("/resource/accountId", *resource.account_id.as_uuid()),
    ] {
        validate_uuid_text(raw, pointer, Some(id))?;
    }
    Ok(())
}

fn kind(value: &impl serde::Serialize) -> Result<String, ArchiveError> {
    Ok(model::encode(value)?.trim_matches('"').to_owned())
}

fn validate_catalog(
    connection: &Connection,
    validators: &BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
) -> Result<(), ArchiveError> {
    // Also validate retained identities without an observation: source removal
    // may leave these when another source still has an optional reference.
    let mut query = connection.prepare("SELECT provider, account_id, resource_id, kind, locator_schema, locator_version, canonical_key, locator_json FROM resource_identities").map_err(storage)?;
    let mut rows = query.query([]).map_err(storage)?;
    while let Some(row) = rows.next().map_err(storage)? {
        let json: String = row.get(7).map_err(storage)?;
        let record: ProviderResourceRef = model::decode(&json)?;
        validate_uuid_text(
            &model::decode(&json)?,
            "/accountId",
            Some(*record.account_id.as_uuid()),
        )?;
        let validator = validators
            .get(&record.provider)
            .ok_or(ArchiveError::InvalidStore)?;
        model::validate_resource(&record, validator.as_ref())?;
        let expected: Vec<Value> = vec![
            record.provider.as_str().to_owned().into(),
            record.account_id.as_uuid().to_string().into(),
            record
                .resource_id()
                .map_err(|_| ArchiveError::InvalidStore)?
                .to_string()
                .into(),
            kind(&record.resource_kind)?.into(),
            record.locator_schema.into(),
            i64::from(record.locator_version).into(),
            record.canonical_key.into(),
        ];
        for (column, expected) in expected.iter().enumerate() {
            if &row.get::<_, Value>(column).map_err(storage)? != expected {
                return Err(ArchiveError::InvalidStore);
            }
        }
    }
    Ok(())
}

fn validate_catalog_reference(
    connection: &Connection,
    resource: &ProviderResourceRef,
) -> Result<(), ArchiveError> {
    let json: String = connection
        .query_row(
            "SELECT locator_json FROM resource_identities WHERE resource_id=?",
            [resource
                .resource_id()
                .map_err(|_| ArchiveError::InvalidRecord)?
                .to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if model::decode::<ProviderResourceRef>(&json)? != *resource {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

fn validate_content_projections(
    connection: &Connection,
    record: &ContentRecord,
) -> Result<(), ArchiveError> {
    let scope = super::ingest_state::scope_sql(&record.scope);
    let id = record.id.as_uuid().to_string();
    let bindings = params![scope[0], scope[1], scope[2], id];
    let names = record
        .attachments
        .iter()
        .filter_map(|attachment| attachment.safe_display_name.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    let expected: Vec<Value> = vec![
        record.conversation_id.as_uuid().to_string().into(),
        record.author_id.as_uuid().to_string().into(),
        record
            .reply_to
            .map(|id| Value::from(id.as_uuid().to_string()))
            .unwrap_or(Value::Null),
        record
            .thread_parent
            .map(|id| Value::from(id.as_uuid().to_string()))
            .unwrap_or(Value::Null),
        record.timestamp.timestamp().into(),
        i64::from(record.timestamp.timestamp_subsec_nanos()).into(),
        record.searchable_text.clone().into(),
        names.into(),
    ];
    let actual: Vec<Value> = connection.query_row("SELECT conversation_id, author_id, reply_to_id, thread_parent_id, timestamp_seconds, timestamp_nanos, searchable_text, attachment_names FROM content_observations WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4", bindings, |row| (0..8).map(|column| row.get(column)).collect()).map_err(storage)?;
    if actual != expected {
        return Err(ArchiveError::InvalidStore);
    }

    let mut query = connection.prepare("SELECT ordinal, record_json FROM attachments WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4 ORDER BY ordinal").map_err(storage)?;
    let mut rows = query.query(bindings).map_err(storage)?;
    for (ordinal, attachment) in record.attachments.iter().enumerate() {
        let row = rows
            .next()
            .map_err(storage)?
            .ok_or(ArchiveError::InvalidStore)?;
        if row.get::<_, i64>(0).map_err(storage)? != ordinal as i64
            || model::decode::<retract_domain::AttachmentRecord>(
                &row.get::<_, String>(1).map_err(storage)?,
            )? != *attachment
        {
            return Err(ArchiveError::InvalidStore);
        }
    }
    if rows.next().map_err(storage)?.is_some() {
        return Err(ArchiveError::InvalidStore);
    }

    let mut expected_findings = BTreeMap::new();
    for finding in &record.privacy_findings {
        let version = record
            .detector_version
            .as_ref()
            .ok_or(ArchiveError::InvalidStore)?;
        if expected_findings.insert(kind(finding)?, version).is_some() {
            return Err(ArchiveError::InvalidStore);
        }
    }
    let mut query = connection.prepare("SELECT kind, detector_version FROM privacy_findings WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4 ORDER BY kind").map_err(storage)?;
    let mut rows = query.query(bindings).map_err(storage)?;
    for (kind, version) in expected_findings {
        let row = rows
            .next()
            .map_err(storage)?
            .ok_or(ArchiveError::InvalidStore)?;
        if row.get::<_, String>(0).map_err(storage)? != kind
            || row.get::<_, String>(1).map_err(storage)? != *version
        {
            return Err(ArchiveError::InvalidStore);
        }
    }
    if rows.next().map_err(storage)?.is_some() {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}
