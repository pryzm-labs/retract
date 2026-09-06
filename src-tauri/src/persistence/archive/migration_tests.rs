use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use super::{
    ArchiveError, ArchiveStore,
    codec::{open_immutable_keyed, open_keyed},
    migration, model, schema,
    test_support::{Fixture, account, key, snapshot_recovery_files, source, validators},
};

const OLD_DDL: &str =
    "CREATE TABLE legacy_source(account_json TEXT NOT NULL, source_json TEXT NOT NULL) STRICT";

fn candidate(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".migration");
    PathBuf::from(name)
}

fn legacy(fixture: &Fixture) {
    super::store::private_options()
        .create_new(true)
        .open(&fixture.path)
        .unwrap();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch(OLD_DDL).unwrap();
    let mut source = source();
    source.archive_fingerprint = Some("migrationcanary".into());
    db.execute(
        "INSERT INTO legacy_source VALUES(?, ?)",
        [
            model::encode(&account()).unwrap(),
            model::encode(&source).unwrap(),
        ],
    )
    .unwrap();
    drop(db);
}

fn validate_old(connection: &Connection) -> Result<(), ArchiveError> {
    let ddl: Vec<String> = connection
        .prepare("SELECT sql FROM sqlite_schema ORDER BY name")
        .map_err(super::ingest_state::storage)?
        .query_map([], |row| row.get(0))
        .map_err(super::ingest_state::storage)?
        .collect::<Result<_, _>>()
        .map_err(super::ingest_state::storage)?;
    if ddl != [OLD_DDL] {
        return Err(ArchiveError::UnsupportedSchema);
    }
    let count: i64 = connection
        .query_row("SELECT count(*) FROM legacy_source", [], |row| row.get(0))
        .map_err(super::ingest_state::storage)?;
    if count != 1 {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

fn populate(old: &Connection, new: &mut Connection) -> Result<(), ArchiveError> {
    let (account_json, source_json): (String, String) = old
        .query_row(
            "SELECT account_json, source_json FROM legacy_source",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(super::ingest_state::storage)?;
    let account: retract_domain::AccountRecord = model::decode(&account_json)?;
    let source: retract_domain::SourceRecord = model::decode(&source_json)?;
    let tx = new.transaction().map_err(super::ingest_state::storage)?;
    tx.execute(
        "INSERT INTO accounts VALUES(?, ?, ?, ?)",
        (
            account.provider.as_str(),
            account.id.as_uuid().to_string(),
            "synthetic:9007199254740992",
            account_json,
        ),
    )
    .map_err(super::ingest_state::storage)?;
    tx.execute(
        "INSERT INTO sources VALUES(?, ?, ?, ?)",
        (
            source.provider.as_str(),
            source.account_id.as_uuid().to_string(),
            source.id.as_uuid().to_string(),
            source_json,
        ),
    )
    .map_err(super::ingest_state::storage)?;
    tx.commit().map_err(super::ingest_state::storage)
}

fn validate_candidate(db: &Connection) -> Result<(), ArchiveError> {
    schema::validate(db)?;
    let count: i64 = db.query_row("SELECT count(*) FROM sources WHERE json_extract(record_json, '$.archiveFingerprint')='migrationcanary'", [], |row| row.get(0)).map_err(super::ingest_state::storage)?;
    if count != 1 {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

#[test]
fn encrypted_candidate_migrates_test_only_old_schema_and_preserves_data() {
    let fixture = Fixture::new();
    legacy(&fixture);
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::UnsupportedSchema)
    ));
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            validate_old,
            populate,
            validate_candidate
        ),
        Ok(())
    );
    let store = fixture.open();
    assert_eq!(
        store
            .source(&source().scope())
            .unwrap()
            .archive_fingerprint
            .as_deref(),
        Some("migrationcanary")
    );
    assert!(!candidate(&fixture.path).exists());
    for (_, bytes) in snapshot_recovery_files(&fixture.path) {
        if let Some(bytes) = bytes {
            assert!(!bytes.windows(15).any(|part| part == b"migrationcanary"));
        }
    }
}

#[test]
fn migration_uses_store_process_lock_before_callbacks_or_candidate_creation() {
    let fixture = Fixture::new();
    let _store = fixture.open();
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            |_| panic!("locked old read"),
            |_, _| panic!("locked copy"),
            |_| panic!("locked validation")
        ),
        Err(ArchiveError::StoreInUse)
    );
    assert!(!candidate(&fixture.path).exists());
}

#[test]
fn candidate_validation_failure_preserves_original_and_retains_encrypted_artifact() {
    let fixture = Fixture::new();
    legacy(&fixture);
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        migration::migrate(&fixture.path, &key(), validate_old, populate, |_| Err(
            ArchiveError::InvalidStore
        )),
        Err(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    let candidate = candidate(&fixture.path);
    assert!(candidate.exists());
    let bytes = fs::read(&candidate).unwrap();
    assert!(!bytes.windows(15).any(|part| part == b"migrationcanary"));
    assert_eq!(
        fs::metadata(&candidate).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            validate_old,
            populate,
            validate_candidate
        ),
        Err(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn valid_active_ignores_obsolete_candidate_and_missing_active_fails_closed() {
    let fixture = Fixture::new();
    legacy(&fixture);
    fs::rename(&fixture.path, candidate(&fixture.path)).unwrap();
    let obsolete = fs::read(candidate(&fixture.path)).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    assert!(!fixture.path.exists());
    // A current active file always wins; candidate data is never promoted or
    // consumed. Unknown candidate files (even symlinks) are not deleted.
    let current = Fixture::new();
    drop(current.open());
    fs::copy(&current.path, &fixture.path).unwrap();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o600)).unwrap();
    let store = fixture.open();
    assert_eq!(
        store.source(&source().scope()),
        Err(ArchiveError::ScopeMismatch)
    );
    drop(store);
    assert_eq!(fs::read(candidate(&fixture.path)).unwrap(), obsolete);
    fs::remove_file(candidate(&fixture.path)).unwrap();
    symlink(&current.path, candidate(&fixture.path)).unwrap();
    drop(fixture.open());
    assert!(
        fs::symlink_metadata(candidate(&fixture.path))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn migration_rejects_wrong_key_unknown_schema_and_recovery_sidecars_without_mutation() {
    let fixture = Fixture::new();
    legacy(&fixture);
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &super::ArchiveKey::new([0x31; 32]),
            validate_old,
            populate,
            validate_candidate
        ),
        Err(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    assert!(!candidate(&fixture.path).exists());
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            |_| Err(ArchiveError::UnsupportedSchema),
            populate,
            validate_candidate
        ),
        Err(ArchiveError::UnsupportedSchema)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    let mut journal = fixture.path.as_os_str().to_owned();
    journal.push("-journal");
    super::store::private_options()
        .create_new(true)
        .open(PathBuf::from(journal))
        .unwrap();
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            validate_old,
            populate,
            validate_candidate
        ),
        Err(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    assert!(!candidate(&fixture.path).exists());
}

#[test]
fn real_candidate_disk_full_and_permission_failures_preserve_old_file() {
    let fixture = Fixture::new();
    legacy(&fixture);
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        migration::migrate(
            &fixture.path,
            &key(),
            validate_old,
            |_, new| {
                let pages: i64 = new
                    .pragma_query_value(None, "page_count", |row| row.get(0))
                    .unwrap();
                new.pragma_update(None, "max_page_count", pages).unwrap();
                let error = new
                    .execute(
                        "INSERT INTO accounts VALUES('synthetic', 'oversized', 'oversized', ?)",
                        ["x".repeat(1024 * 1024)],
                    )
                    .unwrap_err();
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DiskFull)
                );
                Err(ArchiveError::StorageFailure)
            },
            validate_candidate
        ),
        Err(ArchiveError::StorageFailure)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    validate_old(&open_immutable_keyed(&fixture.path, &key()).unwrap()).unwrap();

    let fixture = Fixture::new();
    legacy(&fixture);
    let before = snapshot_recovery_files(&fixture.path);
    let parent = fixture.path.parent().unwrap();
    let result = migration::migrate(
        &fixture.path,
        &key(),
        |old| {
            validate_old(old)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
            Ok(())
        },
        populate,
        validate_candidate,
    );
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, Err(ArchiveError::StorageFailure));
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn real_rename_failure_leaves_old_authoritative_and_candidate_encrypted() {
    let fixture = Fixture::new();
    legacy(&fixture);
    let before = snapshot_recovery_files(&fixture.path);
    let parent = fixture.path.parent().unwrap();
    let result = migration::migrate(&fixture.path, &key(), validate_old, populate, |new| {
        validate_candidate(new)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
        Ok(())
    });
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, Err(ArchiveError::StorageFailure));
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    validate_old(&open_immutable_keyed(&fixture.path, &key()).unwrap()).unwrap();
    validate_candidate(&open_immutable_keyed(&candidate(&fixture.path), &key()).unwrap()).unwrap();
}

#[test]
fn interrupted_candidate_sidecars_are_never_recovered_or_used_as_an_original() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let fixture = Fixture::new();
        legacy(&fixture);
        let before = snapshot_recovery_files(&fixture.path);
        let artifact = super::store::sidecar(&candidate(&fixture.path), suffix);
        fs::write(&artifact, b"unknown synthetic bytes").unwrap();
        fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            migration::migrate(
                &fixture.path,
                &key(),
                validate_old,
                populate,
                validate_candidate
            ),
            Err(ArchiveError::InvalidStore)
        );
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
        assert_eq!(fs::read(&artifact).unwrap(), b"unknown synthetic bytes");
        fs::remove_file(&fixture.path).unwrap();
        assert!(matches!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()),
            Err(ArchiveError::InvalidStore)
        ));
        assert!(!fixture.path.exists());
        assert_eq!(fs::read(&artifact).unwrap(), b"unknown synthetic bytes");
    }
}

#[test]
fn post_switch_sync_failure_never_restores_old_schema_or_leaves_candidate_wal() {
    let fixture = Fixture::new();
    legacy(&fixture);
    migration::FAIL_FINAL_SYNC.set(true);
    let result = migration::migrate(
        &fixture.path,
        &key(),
        validate_old,
        |old, new| {
            new.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            populate(old, new)
        },
        validate_candidate,
    );
    migration::FAIL_FINAL_SYNC.set(false);
    assert_eq!(result, Err(ArchiveError::StorageFailure));
    let store = fixture.open();
    assert_eq!(
        store
            .source(&source().scope())
            .unwrap()
            .archive_fingerprint
            .as_deref(),
        Some("migrationcanary")
    );
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!super::store::sidecar(&candidate(&fixture.path), suffix).exists());
    }
}

#[test]
fn candidate_schema_and_codec_policy_checks_are_mandatory() {
    for tamper in [
        "DROP TRIGGER source_retired_insert",
        "PRAGMA temp_store=FILE",
    ] {
        let fixture = Fixture::new();
        legacy(&fixture);
        let before = snapshot_recovery_files(&fixture.path);
        assert!(
            migration::migrate(
                &fixture.path,
                &key(),
                validate_old,
                |old, new| {
                    populate(old, new)?;
                    new.execute_batch(tamper).unwrap();
                    Ok(())
                },
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
    }
}
