use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use super::{
    ArchiveError,
    codec::{ArchiveKey, open_keyed, validate_codec_metadata, validate_connection_settings},
};

#[test]
fn database_fixture_path_resolves_a_symlinked_temporary_root() {
    let outer = tempfile::tempdir().unwrap();
    let real_root = outer.path().join("real");
    let symlinked_root = outer.path().join("alias");
    fs::create_dir(&real_root).unwrap();
    symlink(&real_root, &symlinked_root).unwrap();

    let path = database_fixture_path(&symlinked_root, "synthetic.db");
    let result = open_keyed(&path, &ArchiveKey::new([0x42; 32]), true);
    assert!(
        result.is_ok(),
        "trusted fixture path did not open: {:?}",
        result.err()
    );
    assert_eq!(
        path.parent().unwrap(),
        fs::canonicalize(symlinked_root).unwrap()
    );
}

fn database_fixture_path(root: &Path, name: &str) -> PathBuf {
    fs::canonicalize(root).unwrap().join(name)
}

#[test]
fn immutable_keyed_probe_of_clean_wal_store_cannot_write_or_create_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "synthetic ?#%.db");
    let key = ArchiveKey::new([0x42; 32]);
    let db = open_keyed(&path, &key, true).unwrap();
    db.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE probe(value TEXT); INSERT INTO probe VALUES('encryptedcanary');").unwrap();
    drop(db);
    let before = fs::read(&path).unwrap();
    let db = super::codec::open_immutable_keyed(&path, &key).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM probe", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "encryptedcanary"
    );
    assert!(
        db.execute("INSERT INTO probe VALUES('forbidden')", [])
            .is_err()
    );
    drop(db);
    assert_eq!(fs::read(&path).unwrap(), before);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        assert!(!PathBuf::from(sidecar).exists());
    }
    assert_eq!(
        super::codec::open_immutable_keyed(&path, &ArchiveKey::new([0x43; 32])).err(),
        Some(ArchiveError::InvalidStore)
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let alias = database_fixture_path(dir.path(), "alias.db");
    symlink(&path, &alias).unwrap();
    assert_eq!(
        super::codec::open_immutable_keyed(&alias, &key).err(),
        Some(ArchiveError::InvalidStore)
    );
}

#[test]
fn keyed_connection_reopens_encrypted_fts_and_rejects_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "synthetic.db");
    let key = ArchiveKey::new([0x42; 32]);
    let db = open_keyed(&path, &key, true).expect("SQLCipher gate must open");
    db.execute_batch(
        "CREATE VIRTUAL TABLE gate_text USING fts5(body);
        INSERT INTO gate_text(gate_text, rank) VALUES('secure-delete', 1);
        INSERT INTO gate_text(body) VALUES('syntheticcanary');",
    )
    .unwrap();
    drop(db);
    assert!(open_keyed(&path, &ArchiveKey::new([0x43; 32]), false).is_err());
    let db = open_keyed(&path, &key, false).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM gate_text WHERE gate_text MATCH 'syntheticcanary'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    db.execute("DELETE FROM gate_text", []).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM gate_text WHERE gate_text MATCH 'syntheticcanary'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

#[test]
fn rejected_plaintext_store_is_preserved_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "plaintext.db");
    let plaintext = Connection::open(&path).unwrap();
    plaintext
        .execute_batch("CREATE TABLE visible(value TEXT); INSERT INTO visible VALUES('canary');")
        .unwrap();
    drop(plaintext);
    let before = fs::read(&path).unwrap();

    assert_eq!(
        open_keyed(&path, &ArchiveKey::new([0x42; 32]), false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn encrypted_database_wal_and_journal_do_not_contain_plaintext_canaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "artifacts.db");
    let db = open_keyed(&path, &ArchiveKey::new([0x42; 32]), true).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE secrets(value TEXT NOT NULL);
         INSERT INTO secrets VALUES('syntheticcanary');",
    )
    .unwrap();

    assert_artifact_hides(&path, b"syntheticcanary");
    assert_artifact_hides(&path.with_extension("db-wal"), b"syntheticcanary");

    db.execute_batch(
        "PRAGMA journal_mode = DELETE;
         BEGIN IMMEDIATE;
         UPDATE secrets SET value = 'replacementcanary';",
    )
    .unwrap();
    let journal_path = path.with_extension("db-journal");
    assert!(journal_path.is_file());
    assert_artifact_hides(&journal_path, b"syntheticcanary");
    assert_artifact_hides(&journal_path, b"replacementcanary");
    db.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn tampered_encrypted_page_is_rejected_without_further_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "tampered.db");
    let key = ArchiveKey::new([0x42; 32]);
    let db = open_keyed(&path, &key, true).unwrap();
    db.execute_batch("CREATE TABLE data(value TEXT); INSERT INTO data VALUES('canary');")
        .unwrap();
    drop(db);

    let mut tampered = fs::read(&path).unwrap();
    assert!(tampered.len() > 32);
    tampered[32] ^= 0x80;
    fs::write(&path, &tampered).unwrap();

    assert_eq!(
        open_keyed(&path, &key, false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert_eq!(fs::read(path).unwrap(), tampered);
}

#[test]
fn wrong_key_rejection_preserves_existing_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "wrong-key.db");
    let db = open_keyed(&path, &ArchiveKey::new([0x42; 32]), true).unwrap();
    db.execute_batch("CREATE TABLE data(value TEXT); INSERT INTO data VALUES('canary');")
        .unwrap();
    drop(db);
    let before = fs::read(&path).unwrap();

    assert_eq!(
        open_keyed(&path, &ArchiveKey::new([0x43; 32]), false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn absent_or_unexpected_codec_metadata_is_rejected() {
    assert_eq!(
        validate_codec_metadata(None, Some("openssl")),
        Err(ArchiveError::UnsupportedCodec)
    );
    assert_eq!(
        validate_codec_metadata(Some("4.14.0 community"), None),
        Err(ArchiveError::UnsupportedCodec)
    );
    assert_eq!(
        validate_codec_metadata(Some("4.14.0 community"), Some("commoncrypto")),
        Err(ArchiveError::UnsupportedCodec)
    );
    assert_eq!(
        validate_codec_metadata(Some("4.13.0 community"), Some("openssl")),
        Err(ArchiveError::UnsupportedCodec)
    );
    assert_eq!(
        validate_codec_metadata(Some("4.14.0 community"), Some("openssl")),
        Ok(())
    );
}

#[test]
fn file_backed_temp_store_configuration_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "temp-store.db");
    let db = open_keyed(&path, &ArchiveKey::new([0x42; 32]), true).unwrap();
    db.execute_batch("PRAGMA temp_store = FILE").unwrap();
    assert_eq!(
        validate_connection_settings(&db),
        Err(ArchiveError::UnsupportedCodec)
    );
}

#[test]
fn extension_loading_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "extensions.db");
    let db = open_keyed(&path, &ArchiveKey::new([0x42; 32]), true).unwrap();
    assert!(
        db.query_row("SELECT load_extension('untrusted')", [], |_| Ok(()))
            .is_err()
    );
}

#[test]
fn missing_noncreating_path_is_rejected_without_creating_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_fixture_path(dir.path(), "missing.db");
    assert_eq!(
        open_keyed(&path, &ArchiveKey::new([0x42; 32]), false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert!(!path.exists());
}

#[test]
fn symlink_and_nonregular_paths_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let target = database_fixture_path(dir.path(), "target.db");
    let link = database_fixture_path(dir.path(), "link.db");
    fs::write(&target, b"sentinel").unwrap();
    symlink(&target, &link).unwrap();

    assert_eq!(
        open_keyed(&link, &ArchiveKey::new([0x42; 32]), false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert_eq!(
        open_keyed(dir.path(), &ArchiveKey::new([0x42; 32]), false).unwrap_err(),
        ArchiveError::InvalidStore
    );
    assert_eq!(fs::read(target).unwrap(), b"sentinel");
}

#[test]
fn archive_errors_expose_only_fixed_safe_codes() {
    let cases = [
        (
            ArchiveError::UnsupportedCodec,
            "unsupported_codec",
            "UnsupportedCodec",
        ),
        (
            ArchiveError::UnavailableKey,
            "unavailable_key",
            "UnavailableKey",
        ),
        (ArchiveError::InvalidStore, "invalid_store", "InvalidStore"),
        (
            ArchiveError::UnsupportedSchema,
            "unsupported_schema",
            "UnsupportedSchema",
        ),
        (ArchiveError::StoreInUse, "store_in_use", "StoreInUse"),
        (
            ArchiveError::ScopeMismatch,
            "scope_mismatch",
            "ScopeMismatch",
        ),
        (
            ArchiveError::InvalidRecord,
            "invalid_record",
            "InvalidRecord",
        ),
        (
            ArchiveError::LimitExceeded,
            "limit_exceeded",
            "LimitExceeded",
        ),
        (
            ArchiveError::IncompleteSource,
            "incomplete_source",
            "IncompleteSource",
        ),
        (ArchiveError::StaleCursor, "stale_cursor", "StaleCursor"),
        (ArchiveError::Cancelled, "cancelled", "Cancelled"),
        (
            ArchiveError::StorageFailure,
            "storage_failure",
            "StorageFailure",
        ),
        (
            ArchiveError::CleanupPending,
            "cleanup_pending",
            "CleanupPending",
        ),
    ];
    for (error, expected_code, expected_debug) in cases {
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.to_string(), expected_code);
        assert_eq!(format!("{error:?}"), expected_debug);
    }
}

fn assert_artifact_hides(path: &std::path::Path, canary: &[u8]) {
    let bytes = fs::read(path).unwrap();
    assert!(
        !bytes.windows(canary.len()).any(|window| window == canary),
        "encrypted artifact contained a plaintext canary"
    );
}
