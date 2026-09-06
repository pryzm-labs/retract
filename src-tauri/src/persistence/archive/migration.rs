//! Guarded encrypted candidate plumbing. No production predecessor is enabled.
//! A candidate never authorizes recovery or replaces an accepted active store.

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
