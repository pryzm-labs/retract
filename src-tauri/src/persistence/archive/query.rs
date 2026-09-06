//! Bounded queries over a single published archive observation set.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use retract_domain::{ContentRecord, ConversationRecord, ResourceKind, Scope};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::providers::ports::{ConversationQuery, Page, ResolveRequest};

use super::{ArchiveError, ArchiveSearch, ArchiveStore, ingest_state, model};

const MAX_PAGE: u32 = 200;
const MAX_QUERY: usize = 4096;
const MAX_CURSOR: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    scope: Scope,
    filter: String,
    run: Uuid,
    revision: u64,
    #[serde(deserialize_with = "required_timestamp")]
    timestamp: Option<DateTime<Utc>>,
    id: Uuid,
}

// A conversation cursor uses explicit null; a missing field is not this schema.
fn required_timestamp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error> {
    Option::deserialize(deserializer)
}

impl ArchiveStore {
    /// Text is a literal phrase in the FTS tokenizer, including punctuation and
    /// quotes. Operator syntax is never accepted. Empty text lists recent items.
    pub(crate) fn search(
        &self,
        request: ArchiveSearch,
    ) -> Result<Page<ContentRecord>, ArchiveError> {
        search_bounds(&request)?;
        let text = request.text.trim();
        let mut kinds = request
            .kinds
            .iter()
            .map(|kind| {
                serde_json::to_value(kind)
                    .map_err(|_| ArchiveError::InvalidRecord)
                    .and_then(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or(ArchiveError::InvalidRecord)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        kinds.sort();
        kinds.dedup();
        let filter = digest(&(
            "content",
            text,
            &kinds,
            request.author,
            request.before,
            request.after,
        ))?;
        let cursor = decode_cursor(request.cursor.as_deref(), &request.scope, &filter, true)?;
        self.transaction(|tx| {
            let (run, revision) = generation(tx, &request.scope, cursor.as_ref())?;
            let mut sql = String::from("SELECT c.record_json FROM content_observations c WHERE c.provider=? AND c.account_id=? AND c.source_id=?");
            let mut values = scope_values(&request.scope);
            if !text.is_empty() {
                sql.push_str(" AND c.observation_key IN (SELECT rowid FROM content_fts WHERE content_fts MATCH ?)");
                values.push(Value::Text(format!("\"{}\"", text.replace('"', "\"\""))));
            }
            if !kinds.is_empty() {
                sql.push_str(" AND json_extract(c.record_json, '$.kind') IN (");
                sql.push_str(&vec!["?"; kinds.len()].join(","));
                sql.push(')');
                values.extend(kinds.iter().cloned().map(Value::Text));
            }
            if let Some(author) = request.author {
                sql.push_str(" AND c.author_id=?");
                values.push(Value::Text(author.as_uuid().to_string()));
            }
            for (date, operator) in [(request.before, "<"), (request.after, ">")] {
                if let Some(date) = date {
                    sql.push_str(&format!(" AND (c.timestamp_seconds, c.timestamp_nanos) {operator} (?,?)"));
                    values.extend(timestamp_values(date));
                }
            }
            if let Some(cursor) = &cursor {
                sql.push_str(" AND (c.timestamp_seconds, c.timestamp_nanos, c.resource_id) < (?,?,?)");
                values.extend(timestamp_values(cursor.timestamp.ok_or(ArchiveError::StaleCursor)?));
                values.push(Value::Text(cursor.id.to_string()));
            }
            sql.push_str(" ORDER BY c.timestamp_seconds DESC, c.timestamp_nanos DESC, c.resource_id DESC LIMIT ?");
            values.push(Value::Integer(i64::from(request.limit) + 1));
            let mut items: Vec<ContentRecord> = records(tx, &sql, values)?;
            let more = items.len() > request.limit as usize;
            items.truncate(request.limit as usize);
            let next_cursor = if more {
                let last = items.last().ok_or(ArchiveError::InvalidStore)?;
                Some(encode_cursor(Cursor { version: 1, scope: request.scope.clone(), filter, run, revision, timestamp: Some(last.timestamp), id: *last.id.as_uuid() })?)
            } else { None };
            Ok(Page { items, next_cursor })
        })
    }

    pub(crate) fn list_conversations(
        &self,
        request: ConversationQuery,
    ) -> Result<Page<ConversationRecord>, ArchiveError> {
        conversation_bounds(&request)?;
        let filter = digest(&"conversations")?;
        let cursor = decode_cursor(request.cursor.as_deref(), &request.scope, &filter, false)?;
        self.transaction(|tx| {
            let (run, revision) = generation(tx, &request.scope, cursor.as_ref())?;
            // Conversations have observation time, not a content timestamp.
            // Their source-scoped primary key provides stable bounded ID order.
            let mut sql = String::from("SELECT record_json FROM conversation_observations WHERE provider=? AND account_id=? AND source_id=?");
            let mut values = scope_values(&request.scope);
            if let Some(cursor) = &cursor {
                sql.push_str(" AND resource_id < ?");
                values.push(Value::Text(cursor.id.to_string()));
            }
            sql.push_str(" ORDER BY resource_id DESC LIMIT ?");
            values.push(Value::Integer(i64::from(request.limit) + 1));
            let mut items: Vec<ConversationRecord> = records(tx, &sql, values)?;
            let more = items.len() > request.limit as usize;
            items.truncate(request.limit as usize);
            let next_cursor = if more {
                let last = items.last().ok_or(ArchiveError::InvalidStore)?;
                Some(encode_cursor(Cursor { version: 1, scope: request.scope.clone(), filter, run, revision, timestamp: None, id: *last.id.as_uuid() })?)
            } else { None };
            Ok(Page { items, next_cursor })
        })
    }

    pub(crate) fn resolve(
        &self,
        request: ResolveRequest,
    ) -> Result<Vec<ContentRecord>, ArchiveError> {
        resolve_bounds(&request)?;
        self.transaction(|tx| {
            generation(tx, &request.scope, None)?;
            let validator = self.validators.get(&request.scope.provider).ok_or(ArchiveError::InvalidRecord)?;
            // Complete validation before any content lookup. Adapter parsing is
            // behind the same bounded envelope check used during ingestion.
            for target in &request.refs {
                if target.scope != request.scope || target.resource.provider != request.scope.provider || target.resource.account_id != request.scope.account_id {
                    return Err(ArchiveError::ScopeMismatch);
                }
                model::validate_resource(&target.resource, validator.as_ref())?;
                if target.resource.resource_kind != ResourceKind::Content {
                    return Err(ArchiveError::InvalidRecord);
                }
                target.validate(&request.scope).map_err(|_| ArchiveError::InvalidRecord)?;
            }
            let mut query = tx.prepare("SELECT record_json FROM content_observations WHERE provider=? AND account_id=? AND source_id=? AND resource_id=?").map_err(storage)?;
            let mut items = Vec::with_capacity(request.refs.len());
            for target in &request.refs {
                let mut values = scope_values(&request.scope);
                values.push(Value::Text(target.id.to_string()));
                let encoded: Option<String> = query.query_row(params_from_iter(values), |row| row.get(0)).optional().map_err(storage)?;
                if let Some(encoded) = encoded {
                    let record: ContentRecord = model::decode(&encoded)?;
                    if record.resource != target.resource { return Err(ArchiveError::InvalidRecord); }
                    items.push(record);
                }
            }
            Ok(items)
        })
    }
}

pub(super) fn search_bounds(request: &ArchiveSearch) -> Result<(), ArchiveError> {
    if request.text.len() > MAX_QUERY || request.kinds.len() > 13 {
        return Err(ArchiveError::LimitExceeded);
    }
    validate_limit(request.limit)?;
    cursor_bounds(request.cursor.as_deref())?;
    if let (Some(after), Some(before)) = (request.after, request.before)
        && after >= before
    {
        return Err(ArchiveError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn conversation_bounds(request: &ConversationQuery) -> Result<(), ArchiveError> {
    cursor_bounds(request.cursor.as_deref())?;
    validate_limit(request.limit)
}

pub(super) fn resolve_bounds(request: &ResolveRequest) -> Result<(), ArchiveError> {
    if request.refs.len() > MAX_PAGE as usize {
        return Err(ArchiveError::LimitExceeded);
    }
    for reference in &request.refs {
        model::encoded_size(&reference.resource, model::ENVELOPE_BYTES)?;
    }
    model::encoded_size(request, model::MAX_BATCH_BYTES)?;
    Ok(())
}

fn cursor_bounds(cursor: Option<&str>) -> Result<(), ArchiveError> {
    if cursor.is_some_and(|value| value.len() > MAX_CURSOR) {
        return Err(ArchiveError::StaleCursor);
    }
    Ok(())
}

fn validate_limit(limit: u32) -> Result<(), ArchiveError> {
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(ArchiveError::LimitExceeded);
    }
    Ok(())
}

fn digest(value: &impl Serialize) -> Result<String, ArchiveError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ArchiveError::InvalidRecord)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn decode_cursor(
    value: Option<&str>,
    scope: &Scope,
    filter: &str,
    content: bool,
) -> Result<Option<Cursor>, ArchiveError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > MAX_CURSOR {
        return Err(ArchiveError::StaleCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ArchiveError::StaleCursor)?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| ArchiveError::StaleCursor)?;
    if cursor.version != 1
        || cursor.scope != *scope
        || cursor.filter != filter
        || cursor.run.is_nil()
        || cursor.id.is_nil()
        || cursor.timestamp.is_some() != content
    {
        return Err(ArchiveError::StaleCursor);
    }
    Ok(Some(cursor))
}

fn encode_cursor(cursor: Cursor) -> Result<String, ArchiveError> {
    let bytes = serde_json::to_vec(&cursor).map_err(|_| ArchiveError::InvalidStore)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn generation(
    connection: &Connection,
    scope: &Scope,
    cursor: Option<&Cursor>,
) -> Result<(Uuid, u64), ArchiveError> {
    ingest_state::require_ready(connection, scope).map_err(|error| {
        if cursor.is_some()
            && matches!(
                error,
                ArchiveError::ScopeMismatch | ArchiveError::IncompleteSource
            )
        {
            ArchiveError::StaleCursor
        } else {
            error
        }
    })?;
    let run = ingest_state::load_run(connection, scope)?.ok_or(ArchiveError::IncompleteSource)?;
    let checkpoint = run.checkpoint;
    if cursor.is_some_and(|cursor| {
        cursor.run != checkpoint.run_id || cursor.revision != checkpoint.revision
    }) {
        return Err(ArchiveError::StaleCursor);
    }
    Ok((checkpoint.run_id, checkpoint.revision))
}

fn scope_values(scope: &Scope) -> Vec<Value> {
    vec![
        Value::Text(scope.provider.as_str().into()),
        Value::Text(scope.account_id.as_uuid().to_string()),
        Value::Text(scope.source_id.as_uuid().to_string()),
    ]
}

fn timestamp_values(date: DateTime<Utc>) -> [Value; 2] {
    [
        Value::Integer(date.timestamp()),
        Value::Integer(i64::from(date.timestamp_subsec_nanos())),
    ]
}

fn records<T: DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<T>, ArchiveError> {
    let mut query = connection.prepare(sql).map_err(storage)?;
    let mut rows = query.query(params_from_iter(values)).map_err(storage)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(storage)? {
        items.push(model::decode(&row.get::<_, String>(0).map_err(storage)?)?);
    }
    Ok(items)
}

fn storage(_: rusqlite::Error) -> ArchiveError {
    ArchiveError::StorageFailure
}
