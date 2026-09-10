//! Guarded encrypted candidate plumbing for the authenticated v1 predecessor.
//! Only a fully validated candidate can atomically replace an accepted original.

use std::{
    fs::{self, File},
    io::ErrorKind,
    path::Path,
};

use rusqlite::Connection;

use super::{ArchiveError, ArchiveKey, codec, preflight, remove, schema, store};

#[cfg(test)]
thread_local! {
    pub(super) static FAIL_FINAL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn migrate(
    path: &Path,
    key: &ArchiveKey,
    validate_old: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
    populate: impl FnOnce(&Connection, &mut Connection) -> Result<(), ArchiveError>,
    validate_candidate: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    store::validate_parent(path)?;
    store::validate_artifacts(path)?;
    let _lock = store::acquire_lock(&store::sidecar(path, ".lock"))?;
    migrate_locked(path, key, validate_old, populate, validate_candidate)
}

fn migrate_locked(
    path: &Path,
    key: &ArchiveKey,
    validate_old: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
    populate: impl FnOnce(&Connection, &mut Connection) -> Result<(), ArchiveError>,
    validate_candidate: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    require_clean(path)?;
    let candidate = store::sidecar(path, ".migration");
    if artifact_exists(&candidate)? {
        return Err(ArchiveError::InvalidStore);
    }

    // Immutable preflight snapshots the original and verifies it again after
    // the callback. No original connection ever permits writes or recovery.
    preflight::validate_existing(path, key, |old| {
        validate_old(old)?;
        store::private_options()
            .create_new(true)
            .open(&candidate)
            .and_then(|file| file.sync_all())
            .map_err(|_| ArchiveError::StorageFailure)?;
        let mut new = codec::open_keyed(&candidate, key, false)?;
        schema::initialize(&mut new)?;
        populate(old, &mut new)?;
        validate_candidate(&new)?;
        validate_encrypted_candidate(&new)?;
        new.execute(
            "INSERT INTO content_fts(content_fts, rank) VALUES('integrity-check', 1)",
            [],
        )
        .map_err(super::ingest_state::storage)?;
        remove::checkpoint(&new)?;
        new.close().map_err(|_| ArchiveError::StorageFailure)?;
        require_clean(&candidate)?;
        File::open(&candidate)
            .and_then(|file| file.sync_all())
            .map_err(|_| ArchiveError::StorageFailure)?;
        sync_parent(path)?;
        preflight::validate_existing(&candidate, key, validate_encrypted_candidate)
    })?;
    // Both readers are closed, candidate is durable and clean, original is
    // unchanged. Atomic rename is the sole commit point. Never roll it back:
    // post-rename directory-fsync uncertainty leaves the active file authoritative.
    require_clean(path)?;
    fs::rename(&candidate, path).map_err(|_| ArchiveError::StorageFailure)?;
    #[cfg(test)]
    if FAIL_FINAL_SYNC.replace(false) {
        return Err(ArchiveError::StorageFailure);
    }
    sync_parent(path)
}

/// Called only while ArchiveStore owns the process lock. The v1 original is
/// authenticated without recovery or writes; all modifications target a new,
/// keyed candidate. The ordinary migration validator gates its sole rename.
pub(super) fn upgrade_v1(
    path: &Path,
    key: &ArchiveKey,
    validators: &std::collections::BTreeMap<
        retract_domain::ProviderKey,
        std::sync::Arc<dyn crate::persistence::ProviderPayloadValidator>,
    >,
) -> Result<(), ArchiveError> {
    migrate_locked(
        path,
        key,
        |old| {
            schema::validate_v1(old)?;
            store::validate_registrations(old, validators)?;
            super::ingest_state::validate_runs_v1(old, validators)
        },
        populate_v2,
        |new| {
            store::validate_registrations(new, validators)?;
            super::ingest_state::validate_runs(new, validators)?;
            validate_observations(new, validators)
        },
    )
}

fn validate_observations(
    connection: &Connection,
    validators: &std::collections::BTreeMap<
        retract_domain::ProviderKey,
        std::sync::Arc<dyn crate::persistence::ProviderPayloadValidator>,
    >,
) -> Result<(), ArchiveError> {
    use super::{ingest_state::storage, model};
    use retract_domain::{ActorRecord, ContentRecord, ConversationRecord};
    for table in [
        "actor_observations",
        "conversation_observations",
        "content_observations",
    ] {
        let mut query = connection
            .prepare(&format!(
                "SELECT provider, account_id, source_id, resource_id, record_json FROM {table}"
            ))
            .map_err(storage)?;
        let mut rows = query.query([]).map_err(storage)?;
        while let Some(row) = rows.next().map_err(storage)? {
            let json: String = row.get(4).map_err(storage)?;
            let mut batch = model::ImportBatch::default();
            let (scope, id) = match table {
                "actor_observations" => {
                    let record: ActorRecord = model::decode(&json)?;
                    let out = (record.scope.clone(), *record.id.as_uuid());
                    batch.actors.push(record);
                    out
                }
                "conversation_observations" => {
                    let record: ConversationRecord = model::decode(&json)?;
                    let out = (record.scope.clone(), *record.id.as_uuid());
                    batch.conversations.push(record);
                    out
                }
                _ => {
                    let record: ContentRecord = model::decode(&json)?;
                    let out = (record.scope.clone(), *record.id.as_uuid());
                    batch.contents.push(record);
                    out
                }
            };
            if row.get::<_, String>(0).map_err(storage)? != scope.provider.as_str()
                || row.get::<_, String>(1).map_err(storage)?
                    != scope.account_id.as_uuid().to_string()
                || row.get::<_, String>(2).map_err(storage)?
                    != scope.source_id.as_uuid().to_string()
                || row.get::<_, String>(3).map_err(storage)? != id.to_string()
            {
                return Err(ArchiveError::InvalidStore);
            }
            // The batch's encoded input ceiling excludes store-derived findings.
            // Preserve persisted findings and their detector version verbatim.
            let validator = validators
                .get(&scope.provider)
                .ok_or(ArchiveError::InvalidRecord)?;
            batch.validate(&scope, validator.as_ref())?;
        }
    }
    Ok(())
}

fn populate_v2(old: &Connection, new: &mut Connection) -> Result<(), ArchiveError> {
    use super::ingest_state::storage;
    let tx = new.transaction().map_err(storage)?;
    // Tombstones deliberately outlive their sources. Disable only their insert
    // provenance trigger on this empty candidate and restore its exact DDL in
    // the same transaction, before mandatory schema fingerprint validation.
    let cleanup_trigger: String = tx
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='cleanup_scope_insert'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    tx.execute_batch("DROP TRIGGER cleanup_scope_insert")
        .map_err(storage)?;
    for table in [
        "accounts",
        "sources",
        "resource_identities",
        "conversation_observations",
        "actor_observations",
        "content_observations",
        "attachments",
        "privacy_findings",
        "import_runs",
        "import_batch_receipts",
        "import_warnings",
        "cleanup_tasks",
    ] {
        let mut query = old
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .map_err(storage)?;
        let width = query.column_count();
        let columns = query.column_names().join(", ");
        let placeholders = vec!["?"; width].join(", ");
        let extra = if table == "import_runs" {
            ", observed_at, failure_code"
        } else {
            ""
        };
        let values_extra = if table == "import_runs" { ", ?, ?" } else { "" };
        let mut insert = tx
            .prepare(&format!(
                "INSERT INTO {table}({columns}{extra}) VALUES({placeholders}{values_extra})"
            ))
            .map_err(storage)?;
        let mut rows = query.query([]).map_err(storage)?;
        while let Some(row) = rows.next().map_err(storage)? {
            let mut values = (0..width)
                .map(|i| row.get::<_, rusqlite::types::Value>(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            if table == "import_runs" {
                let source_id: String = row.get(2).map_err(storage)?;
                let observed: String = old.query_row("SELECT json_extract(record_json, '$.updatedAt') FROM sources WHERE source_id=?", [source_id], |r| r.get(0)).map_err(storage)?;
                values.push(observed.into());
                values.push(if row.get::<_, String>(9).map_err(storage)? == "failed" {
                    "incomplete_source".to_owned().into()
                } else {
                    rusqlite::types::Value::Null
                });
            }
            insert
                .execute(rusqlite::params_from_iter(values))
                .map_err(storage)?;
        }
        drop(rows);
        // Verify every original column, including row keys/generations, without
        // retaining the corpus or exposing SQL through the worker boundary.
        let mut copied = tx
            .prepare(&format!("SELECT {columns} FROM {table} ORDER BY rowid"))
            .map_err(storage)?;
        let mut original_rows = query.query([]).map_err(storage)?;
        let mut copied_rows = copied.query([]).map_err(storage)?;
        while let Some(original) = original_rows.next().map_err(storage)? {
            let copy = copied_rows
                .next()
                .map_err(storage)?
                .ok_or(ArchiveError::InvalidStore)?;
            for i in 0..width {
                if original
                    .get::<_, rusqlite::types::Value>(i)
                    .map_err(storage)?
                    != copy.get::<_, rusqlite::types::Value>(i).map_err(storage)?
                {
                    return Err(ArchiveError::InvalidStore);
                }
            }
        }
        if copied_rows.next().map_err(storage)?.is_some() {
            return Err(ArchiveError::InvalidStore);
        }
    }
    tx.execute_batch(&cleanup_trigger).map_err(storage)?;
    tx.commit().map_err(storage)
}

fn validate_encrypted_candidate(connection: &Connection) -> Result<(), ArchiveError> {
    codec::validate_connection_settings(connection)?;
    schema::validate(connection)?;
    let corrupt = connection
        .prepare("PRAGMA cipher_integrity_check")
        .map_err(|_| ArchiveError::InvalidStore)?
        .exists([])
        .map_err(|_| ArchiveError::InvalidStore)?;
    if corrupt {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

// Normal current-schema recovery is unchanged. Guard only the missing-active
// case: interrupted artifacts must never cause creation of a blank replacement.
// With a valid active file, leave unknown/obsolete candidates untouched.
pub(super) fn validate_active_presence(path: &Path) -> Result<(), ArchiveError> {
    if !exists(path)? && artifact_exists(&store::sidecar(path, ".migration"))? {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

fn require_clean(path: &Path) -> Result<(), ArchiveError> {
    store::validate_artifacts(path)?;
    if !exists(path)? {
        return Err(ArchiveError::InvalidStore);
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        if exists(&store::sidecar(path, suffix))? {
            return Err(ArchiveError::InvalidStore);
        }
    }
    Ok(())
}

fn artifact_exists(path: &Path) -> Result<bool, ArchiveError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        if exists(&store::sidecar(path, suffix))? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exists(path: &Path) -> Result<bool, ArchiveError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ArchiveError::StorageFailure),
    }
}

fn sync_parent(path: &Path) -> Result<(), ArchiveError> {
    File::open(path.parent().ok_or(ArchiveError::InvalidStore)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ArchiveError::StorageFailure)
}
