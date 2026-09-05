use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use retract_domain::{ConnectionState, SourceKind, SourceState};
use rusqlite::params;
use serde_json::json;
use uuid::Uuid;

use super::{
    ArchiveError, ArchiveKey, ArchiveStore,
    codec::open_keyed,
    test_support::{
        Fixture, account, duplicate_lock_description, key, resource, source, validators,
    },
};

fn sql(error: rusqlite::Error) -> ArchiveError {
    let _ = error;
    ArchiveError::StorageFailure
}

#[test]
fn create_reopen_preserves_incomplete_source_and_neighboring_jobs() {
    let fixture = Fixture::new();
    let jobs = fixture.directory.path().join("jobs.enc");
    fs::write(&jobs, b"synthetic neighboring job ciphertext").unwrap();
    let store = fixture.open();
    let registered = store.register_source(account(), source()).unwrap();
    assert_eq!(registered.state, SourceState::Preparing);
    drop(store);
    let reopened = fixture.open();
    assert_eq!(reopened.source(&source().scope()).unwrap(), source());
    assert_eq!(
        fs::read(jobs).unwrap(),
        b"synthetic neighboring job ciphertext"
    );
    let bytes = fs::read(&fixture.path).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(
        !bytes
            .windows(24)
            .any(|part| part == b"Synthetic archive account")
    );
    assert_eq!(
        fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn registration_is_idempotent_and_rejects_conflicting_account_and_source_uuids() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    assert_eq!(
        store.register_source(account(), source()).unwrap(),
        source()
    );
    let mut duplicate = account();
    duplicate.id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
        .unwrap()
        .try_into()
        .unwrap();
    duplicate.native_identity.payload = json!({"nativeId": "20000000000000", "encoding": "hex"});
    let mut other = source();
    other.account_id = duplicate.id;
    other.id = Uuid::new_v4().try_into().unwrap();
    assert_eq!(
        store.register_source(duplicate, other),
        Err(ArchiveError::InvalidRecord)
    );
    let mut changed_account = account();
    changed_account.native_identity.payload["nativeId"] = json!("9007199254740993");
    assert_eq!(
        store.register_source(changed_account, source()),
        Err(ArchiveError::InvalidRecord)
    );
    let mut changed_source = source();
    changed_source.archive_fingerprint = Some("different-export".into());
    assert_eq!(
        store.register_source(account(), changed_source),
        Err(ArchiveError::InvalidRecord)
    );
    assert_eq!(store.source(&source().scope()).unwrap(), source());
}

#[test]
fn registration_rejects_live_ready_accounts_foreign_scope_and_unvalidated_payloads() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut live = source();
    live.kind = SourceKind::LiveConnection;
    live.archive_fingerprint = None;
    assert_eq!(
        store.register_source(account(), live),
        Err(ArchiveError::InvalidRecord)
    );
    let mut active_account = account();
    active_account.connection_state = ConnectionState::Ready;
    assert_eq!(
        store.register_source(active_account, source()),
        Err(ArchiveError::InvalidRecord)
    );
    let mut premature = source();
    premature.state = SourceState::Ready;
    assert_eq!(
        store.register_source(account(), premature),
        Err(ArchiveError::InvalidRecord)
    );
    let mut foreign = source();
    foreign.account_id = Uuid::new_v4().try_into().unwrap();
    assert_eq!(
        store.register_source(account(), foreign),
        Err(ArchiveError::ScopeMismatch)
    );
    let mut malformed = source();
    malformed.schema_profile.version = 2;
    assert_eq!(
        store.register_source(account(), malformed),
        Err(ArchiveError::InvalidRecord)
    );
    let count = store.transaction(|tx| {
        tx.query_row("SELECT (SELECT count(*) FROM accounts) + (SELECT count(*) FROM sources) + (SELECT count(*) FROM content_observations)", [], |row| row.get::<_, i64>(0)).map_err(sql)
    }).unwrap();
    assert_eq!(count, 0);
    drop(store);
    let store = ArchiveStore::open(fixture.path, key(), BTreeMap::new()).unwrap();
    assert_eq!(
        store.register_source(account(), source()),
        Err(ArchiveError::InvalidRecord)
    );
}

#[test]
fn repeat_registration_preserves_the_persisted_import_lifecycle() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    let mut completed = source();
    completed.state = SourceState::Ready;
    completed.imported_at = Some(completed.updated_at);
    store
        .transaction(|tx| {
            tx.execute(
                "UPDATE sources SET record_json = ?",
                [serde_json::to_string(&completed).unwrap()],
            )
            .map_err(sql)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.register_source(account(), source()).unwrap(),
        completed
    );
}

#[test]
fn every_source_lookup_binds_provider_account_and_source() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    let mut scope = source().scope();
    scope.provider = "other".to_owned().try_into().unwrap();
    assert_eq!(store.source(&scope), Err(ArchiveError::ScopeMismatch));
    scope = source().scope();
    scope.account_id = Uuid::new_v4().try_into().unwrap();
    assert_eq!(store.source(&scope), Err(ArchiveError::ScopeMismatch));
    scope = source().scope();
    scope.source_id = Uuid::new_v4().try_into().unwrap();
    assert_eq!(store.source(&scope), Err(ArchiveError::ScopeMismatch));
}

#[test]
fn wrong_key_unsupported_version_binding_and_corrupt_files_are_preserved() {
    let fixture = Fixture::new();
    drop(fixture.open());
    let before = fs::read(&fixture.path).unwrap();
    assert_eq!(
        ArchiveStore::open(
            fixture.path.clone(),
            ArchiveKey::new([0x85; 32]),
            validators()
        )
        .err(),
        Some(ArchiveError::InvalidStore)
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    for statement in [
        "UPDATE schema_migrations SET version = 2",
        "UPDATE schema_migrations SET version = 1, application = 'another-application'",
    ] {
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute_batch(statement).unwrap();
        drop(db);
        let before = fs::read(&fixture.path).unwrap();
        assert!(matches!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()),
            Err(ArchiveError::UnsupportedSchema)
        ));
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
    }
    fs::write(&fixture.path, b"synthetic corrupt database").unwrap();
    let before = fs::read(&fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn self_consistent_older_development_schema_is_rejected_without_mutation() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    drop(fixture.open());
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch("DROP INDEX content_scope_author_order; DROP INDEX content_scope_kind_order;")
        .unwrap();
    let mut digest = Sha256::new();
    {
        let mut query = db.prepare("SELECT type, name, tbl_name, coalesce(sql, '') FROM sqlite_schema ORDER BY type, name").unwrap();
        let mut rows = query.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for column in 0..4 {
                let value: String = row.get(column).unwrap();
                digest.update((value.len() as u64).to_be_bytes());
                digest.update(value.as_bytes());
            }
        }
    }
    let hash: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    db.execute("UPDATE schema_migrations SET schema_hash=?", [hash])
        .unwrap();
    drop(db);
    let before = super::test_support::snapshot_recovery_files(&fixture.path);
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::UnsupportedSchema)
    ));
    assert_eq!(
        super::test_support::snapshot_recovery_files(&fixture.path),
        before
    );
}

#[test]
fn existing_empty_or_unrelated_keyed_files_are_not_initialized() {
    let fixture = Fixture::new();
    fs::write(&fixture.path, []).unwrap();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"");
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch("CREATE TABLE unrelated(value TEXT)")
        .unwrap();
    drop(db);
    let before = fs::read(&fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::UnsupportedSchema)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn reopen_requires_current_adapter_validation_for_every_registration() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    drop(store);
    let before = fs::read(&fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), BTreeMap::new()),
        Err(ArchiveError::InvalidRecord)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    let mut malformed = source();
    malformed.schema_profile.version = 2;
    db.execute(
        "UPDATE sources SET record_json = ?",
        [serde_json::to_string(&malformed).unwrap()],
    )
    .unwrap();
    drop(db);
    let before = fs::read(&fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidRecord)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn symlink_insecure_file_parent_and_sidecar_paths_are_rejected_without_mutation() {
    let fixture = Fixture::new();
    let target = fixture.directory.path().join("target");
    fs::write(&target, b"sentinel").unwrap();
    symlink(&target, &fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    assert_eq!(fs::read(&target).unwrap(), b"sentinel");
    fs::remove_file(&fixture.path).unwrap();
    drop(fixture.open());
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::read(&fixture.path).unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o600)).unwrap();
    for suffix in ["-wal", "-shm", "-journal", ".lock"] {
        let sidecar = fixture.path.with_file_name(format!("archive.db{suffix}"));
        if sidecar.exists() {
            fs::remove_file(&sidecar).unwrap();
        }
        symlink(&target, &sidecar).unwrap();
        assert!(matches!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()),
            Err(ArchiveError::InvalidStore)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"sentinel");
        fs::remove_file(sidecar).unwrap();
    }
    let alias = fixture.directory.path().join("alias");
    symlink(fixture.path.parent().unwrap(), &alias).unwrap();
    assert!(matches!(
        ArchiveStore::open(alias.join("archive.db"), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
    fs::set_permissions(
        fixture.path.parent().unwrap(),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(matches!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()),
        Err(ArchiveError::InvalidStore)
    ));
}

#[test]
fn process_lock_precedes_key_loader_and_lasts_until_store_drop() {
    const CHILD_PATH: &str = "RETRACT_ARCHIVE_LOCK_TEST_PATH";
    const CHILD_EXPECTATION: &str = "RETRACT_ARCHIVE_LOCK_TEST_EXPECTATION";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let loads = AtomicUsize::new(0);
        let result = ArchiveStore::open_with_key_loader(path.into(), validators(), || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok(key())
        });
        if std::env::var(CHILD_EXPECTATION).unwrap() == "locked" {
            assert!(matches!(result, Err(ArchiveError::StoreInUse)));
            assert_eq!(loads.load(Ordering::SeqCst), 0);
        } else {
            assert!(result.is_ok());
            assert_eq!(loads.load(Ordering::SeqCst), 1);
        }
        return;
    }
    let fixture = Fixture::new();
    let store = fixture.open();
    let run_child = |expectation| {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "persistence::archive::store_tests::process_lock_precedes_key_loader_and_lasts_until_store_drop", "--nocapture"])
            .env(CHILD_PATH, &fixture.path).env(CHILD_EXPECTATION, expectation)
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!(
                    "archive lock child timed out: {:?}",
                    child.wait_with_output()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "archive lock child failed: {output:?}"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
    };
    run_child("locked");
    assert!(matches!(
        ArchiveStore::open_with_key_loader(fixture.path.clone(), validators(), || panic!(
            "locked store read key"
        )),
        Err(ArchiveError::StoreInUse)
    ));
    drop(store);
    run_child("released");
}

fn insert_resource(
    tx: &rusqlite::Transaction<'_>,
    native_id: &str,
) -> Result<String, ArchiveError> {
    let reference = resource(native_id);
    let id = reference.resource_id().unwrap().to_string();
    tx.execute("INSERT INTO resource_identities(provider, account_id, resource_id, kind, locator_schema, locator_version, canonical_key, locator_json) VALUES('synthetic', '11111111-1111-4111-8111-111111111111', ?, 'content', 'synthetic.content', 1, ?, ?)", params![id, native_id, serde_json::to_string(&reference).unwrap()]).map_err(sql)?;
    Ok(id)
}

fn insert_observation(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    body: &str,
) -> Result<(), ArchiveError> {
    tx.execute("INSERT INTO content_observations(provider, account_id, source_id, resource_id, searchable_text, record_json) VALUES('synthetic', '11111111-1111-4111-8111-111111111111', '22222222-2222-4222-8222-222222222222', ?, ?, '{}')", params![id, body]).map_err(sql)?;
    Ok(())
}

#[test]
fn account_scoped_resource_ids_preserve_native_strings_and_source_observations() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    store.transaction(|tx| {
        for native in ["9007199254740992", "9007199254740993", "message:part/0007"] {
            let id = insert_resource(tx, native)?;
            insert_observation(tx, &id, "syntheticneedle")?;
        }
        let keys = tx.prepare("SELECT canonical_key FROM resource_identities ORDER BY canonical_key").map_err(sql)?
            .query_map([], |r| r.get::<_, String>(0)).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql)?;
        assert_eq!(keys, ["9007199254740992", "9007199254740993", "message:part/0007"]);
        assert!(tx.execute("UPDATE content_observations SET account_id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'", []).is_err());
        assert!(tx.execute("UPDATE content_observations SET provider = 'other'", []).is_err());
        assert!(tx.execute("UPDATE content_observations SET source_id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'", []).is_err());
        Ok(())
    }).unwrap();
}

#[test]
fn fts_updates_and_deletes_commit_or_rollback_with_observations() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    let id = store
        .transaction(|tx| insert_resource(tx, "message:part/0007"))
        .unwrap();
    assert_eq!(
        store.transaction::<()>(|tx| {
            insert_observation(tx, &id, "rolledbackneedle")?;
            Err(ArchiveError::Cancelled)
        }),
        Err(ArchiveError::Cancelled)
    );
    store
        .transaction(|tx| {
            assert_eq!(
                tx.query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH 'rolledbackneedle'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .map_err(sql)?,
                0
            );
            insert_observation(tx, &id, "firstneedle")?;
            assert_eq!(
                tx.query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH 'firstneedle'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .map_err(sql)?,
                1
            );
            tx.execute(
                "UPDATE content_observations SET searchable_text = 'secondneedle'",
                [],
            )
            .map_err(sql)?;
            assert_eq!(
                tx.query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH 'firstneedle'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .map_err(sql)?,
                0
            );
            assert_eq!(
                tx.query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH 'secondneedle'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .map_err(sql)?,
                1
            );
            tx.execute("DELETE FROM content_observations", [])
                .map_err(sql)?;
            assert_eq!(
                tx.query_row(
                    "SELECT count(*) FROM content_fts WHERE content_fts MATCH 'secondneedle'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .map_err(sql)?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn deleting_one_source_preserves_shared_identity_other_observation_and_fts() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    let mut second = source();
    second.id = Uuid::parse_str("33333333-3333-4333-8333-333333333333")
        .unwrap()
        .try_into()
        .unwrap();
    store.register_source(account(), second).unwrap();
    store.transaction(|tx| {
        let id = insert_resource(tx, "message:part/0007")?;
        insert_observation(tx, &id, "firstsource")?;
        tx.execute("INSERT INTO content_observations(provider, account_id, source_id, resource_id, searchable_text, record_json) VALUES('synthetic', '11111111-1111-4111-8111-111111111111', '33333333-3333-4333-8333-333333333333', ?, 'survivingsource', '{}')", [&id]).map_err(sql)?;
        tx.execute("DELETE FROM sources WHERE source_id = '22222222-2222-4222-8222-222222222222'", []).map_err(sql)?;
        assert_eq!(tx.query_row("SELECT count(*) FROM resource_identities", [], |r| r.get::<_, i64>(0)).map_err(sql)?, 1);
        assert_eq!(tx.query_row("SELECT count(*) FROM content_observations", [], |r| r.get::<_, i64>(0)).map_err(sql)?, 1);
        assert_eq!(tx.query_row("SELECT count(*) FROM content_fts WHERE content_fts MATCH 'firstsource'", [], |r| r.get::<_, i64>(0)).map_err(sql)?, 0);
        assert_eq!(tx.query_row("SELECT count(*) FROM content_fts WHERE content_fts MATCH 'survivingsource'", [], |r| r.get::<_, i64>(0)).map_err(sql)?, 1);
        Ok(())
    }).unwrap();
}

#[test]
fn deferred_references_allow_missing_observations_but_reject_proven_foreign_owners() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    let mut foreign_account = account();
    foreign_account.id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
        .unwrap()
        .try_into()
        .unwrap();
    foreign_account.native_identity.payload["nativeId"] = json!("9007199254740993");
    let mut foreign_source = source();
    foreign_source.id = Uuid::new_v4().try_into().unwrap();
    foreign_source.account_id = foreign_account.id;
    store
        .register_source(foreign_account, foreign_source)
        .unwrap();
    store.transaction(|tx| {
        let id = insert_resource(tx, "message:part/0007")?;
        insert_observation(tx, &id, "deferred")?;
        tx.execute("UPDATE content_observations SET author_id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', reply_to_id = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'", []).map_err(sql)?;
        // An imported reference can precede the identity/observation; resolution
        // later cannot prove that same UUID belongs to a different account.
        assert!(tx.execute("INSERT INTO resource_identities(provider, account_id, resource_id, kind, locator_schema, locator_version, canonical_key, locator_json) VALUES('synthetic', 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', 'actor', 'synthetic.actor', 1, 'foreign', '{}')", []).is_err());
        tx.execute("INSERT INTO resource_identities(provider, account_id, resource_id, kind, locator_schema, locator_version, canonical_key, locator_json) VALUES('synthetic', 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'dddddddd-dddd-4ddd-8ddd-dddddddddddd', 'actor', 'synthetic.actor', 1, 'foreign', '{}')", []).map_err(sql)?;
        for field in ["author_id", "reply_to_id", "conversation_id", "thread_parent_id"] {
            assert!(tx.execute(&format!("UPDATE content_observations SET {field} = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd'"), []).is_err());
        }
        Ok(())
    }).unwrap();
}

#[test]
fn observation_tables_reject_resources_of_the_wrong_kind() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    store.transaction(|tx| {
        let id = insert_resource(tx, "message:part/0007")?;
        for table in ["conversation_observations", "actor_observations"] {
            assert!(tx.execute(&format!("INSERT INTO {table}(provider, account_id, source_id, resource_id, record_json) VALUES('synthetic', '11111111-1111-4111-8111-111111111111', '22222222-2222-4222-8222-222222222222', ?, '{{}}')"), [&id]).is_err());
        }
        Ok(())
    }).unwrap();
}

#[test]
fn missing_schema_triggers_and_disabled_fts_secure_delete_fail_closed() {
    for mutation in [
        "DROP TRIGGER content_fts_insert",
        "UPDATE content_fts_config SET v = 0 WHERE k = 'secure-delete'",
    ] {
        let fixture = Fixture::new();
        drop(fixture.open());
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        let before = fs::read(&fixture.path).unwrap();
        assert!(ArchiveStore::open(fixture.path.clone(), key(), validators()).is_err());
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
    }
}

#[test]
fn dropped_store_releases_its_lock_even_with_a_retained_file_description() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let inherited_description = duplicate_lock_description(&store);
    drop(store);
    let reopened = fixture.open();
    reopened.register_source(account(), source()).unwrap();
    assert_eq!(reopened.source(&source().scope()).unwrap(), source());
    drop(inherited_description);
}

#[test]
fn cleanup_tombstones_bind_original_scope_and_survive_source_deletion() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    store.transaction(|tx| {
        for tuple in [
            ("other", "11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"),
            ("synthetic", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "22222222-2222-4222-8222-222222222222"),
            ("synthetic", "11111111-1111-4111-8111-111111111111", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ] {
            assert!(tx.execute("INSERT INTO cleanup_tasks(task_id, provider, account_id, source_id, state) VALUES('44444444-4444-4444-8444-444444444444', ?, ?, ?, 'pending')", tuple).is_err());
        }
        tx.execute("INSERT INTO cleanup_tasks(task_id, provider, account_id, source_id, state) VALUES('44444444-4444-4444-8444-444444444444', 'synthetic', '11111111-1111-4111-8111-111111111111', '22222222-2222-4222-8222-222222222222', 'pending')", []).map_err(sql)?;
        tx.execute("DELETE FROM sources", []).map_err(sql)?;
        assert_eq!(tx.query_row("SELECT count(*) FROM cleanup_tasks", [], |row| row.get::<_, i64>(0)).map_err(sql)?, 1);
        tx.execute("UPDATE cleanup_tasks SET state = 'compacting'", []).map_err(sql)?;
        for field in ["provider", "account_id", "source_id"] {
            assert!(tx.execute(&format!("UPDATE cleanup_tasks SET {field} = 'foreign'"), []).is_err());
        }
        Ok(())
    }).unwrap();
}

#[test]
fn oversized_registration_envelopes_are_rejected_before_adapter_interpretation() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let mut oversized = account();
    oversized.native_identity.payload["nativeId"] = json!("9".repeat(65536));
    assert_eq!(
        store.register_source(oversized, source()),
        Err(ArchiveError::LimitExceeded)
    );
    let mut oversized = account();
    oversized.avatar = Some(retract_domain::VersionedPayload {
        schema: "synthetic.avatar".into(),
        version: 1,
        payload: json!("x".repeat(65536)),
    });
    assert_eq!(
        store.register_source(oversized, source()),
        Err(ArchiveError::LimitExceeded)
    );
    let mut oversized = source();
    oversized.schema_profile.payload["format"] = json!("x".repeat(65536));
    assert_eq!(
        store.register_source(account(), oversized),
        Err(ArchiveError::LimitExceeded)
    );
    assert_eq!(
        store.source(&source().scope()),
        Err(ArchiveError::ScopeMismatch)
    );
}

#[test]
fn encoded_byte_ceiling_counts_json_quotes_utf8_and_escaping() {
    assert_eq!(
        super::model::encoded_size(&"x".repeat(65534), 65536),
        Ok(65536)
    );
    assert_eq!(
        super::model::encoded_size(&"x".repeat(65535), 65536),
        Err(ArchiveError::LimitExceeded)
    );
    assert_eq!(super::model::encoded_size(&"🦀", 6), Ok(6));
    assert_eq!(super::model::encoded_size(&"\n\n", 6), Ok(6));
    assert_eq!(
        super::model::encoded_size(&"\n\n", 5),
        Err(ArchiveError::LimitExceeded)
    );
}

#[test]
fn reused_account_validation_uses_retained_metadata_and_remains_reopenable() {
    use super::test_support::account_dependent_validators;

    let fixture = Fixture::new();
    let store =
        ArchiveStore::open(fixture.path.clone(), key(), account_dependent_validators()).unwrap();
    let mut original = source();
    original.schema_profile.payload = json!({"accountName": "Synthetic archive account"});
    store.register_source(account(), original.clone()).unwrap();
    let mut changed_account = account();
    changed_account.display_name = "Caller replacement metadata".into();
    let mut conflicting = original.clone();
    conflicting.id = Uuid::new_v4().try_into().unwrap();
    conflicting.schema_profile.payload = json!({"accountName": "Caller replacement metadata"});
    assert_eq!(
        store.register_source(changed_account.clone(), conflicting),
        Err(ArchiveError::InvalidRecord)
    );
    // The new source is valid for the actual retained account. The supplied
    // account's matching canonical identity does not implicitly update metadata.
    let mut compatible = original.clone();
    compatible.id = Uuid::new_v4().try_into().unwrap();
    store
        .register_source(changed_account, compatible.clone())
        .unwrap();
    drop(store);
    let reopened =
        ArchiveStore::open(fixture.path.clone(), key(), account_dependent_validators()).unwrap();
    assert_eq!(reopened.source(&original.scope()).unwrap(), original);
    assert_eq!(reopened.source(&compatible.scope()).unwrap(), compatible);
}

#[test]
fn recovery_fixture_child() {
    const CHILD_PATH: &str = "RETRACT_ARCHIVE_RECOVERY_TEST_PATH";
    const CHILD_MODE: &str = "RETRACT_ARCHIVE_RECOVERY_TEST_MODE";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let db = open_keyed(std::path::Path::new(&path), &key(), false).unwrap();
        match std::env::var(CHILD_MODE).unwrap().as_str() {
            "unsupported-wal" => db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0; UPDATE schema_migrations SET version = 2;").unwrap(),
            "supported-wal" => {
                db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;").unwrap();
                let mut updated = source();
                updated.state = SourceState::Failed;
                db.execute("UPDATE sources SET record_json = ?", [serde_json::to_string(&updated).unwrap()]).unwrap();
            }
            mode @ ("unsupported-journal" | "supported-journal") => {
                if mode == "unsupported-journal" {
                    db.execute_batch("UPDATE schema_migrations SET version = 2;").unwrap();
                }
                db.execute_batch("PRAGMA cache_size = 1; PRAGMA cache_spill = ON; BEGIN IMMEDIATE;").unwrap();
                db.execute("UPDATE schema_migrations SET application = ?", ["synthetic-uncommitted".repeat(100000)]).unwrap();
            }
            mode => panic!("unexpected recovery mode: {mode}"),
        }
        // Emulate a process stopping with durable SQLite recovery artifacts.
        // This child owns no user files or credentials and skips connection drop.
        std::process::exit(0);
    }
}

fn recovery_fixture(mode: &str) -> Fixture {
    use super::test_support::snapshot_recovery_files;
    let fixture = Fixture::new();
    let store = fixture.open();
    store.register_source(account(), source()).unwrap();
    drop(store);
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "persistence::archive::store_tests::recovery_fixture_child",
            "--nocapture",
        ])
        .env("RETRACT_ARCHIVE_RECOVERY_TEST_PATH", &fixture.path)
        .env("RETRACT_ARCHIVE_RECOVERY_TEST_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "synthetic recovery child failed: {output:?}"
    );
    let before = snapshot_recovery_files(&fixture.path);
    assert!(before.iter().any(|(suffix, bytes)| suffix
        == if mode.ends_with("wal") {
            "-wal"
        } else {
            "-journal"
        }
        && bytes.as_ref().is_some_and(|bytes| bytes.len() > 512)));
    fixture
}

fn assert_rejected_recovery_preserved(mode: &str) {
    use super::test_support::snapshot_recovery_files;
    let fixture = recovery_fixture(mode);
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()).err(),
        Some(ArchiveError::UnsupportedSchema)
    );
    let after = snapshot_recovery_files(&fixture.path);
    for ((suffix, before), (_, after)) in before.iter().zip(after.iter()) {
        assert!(
            before == after,
            "rejected {mode} store changed {suffix:?}: original bytes {}, remaining bytes {}",
            before.as_ref().map_or(0, Vec::len),
            after.as_ref().map_or(0, Vec::len)
        );
    }
}

#[test]
fn rejected_wal_store_preserves_original_database_and_sidecars() {
    assert_rejected_recovery_preserved("unsupported-wal");
}

#[test]
fn rejected_hot_journal_store_preserves_original_database_and_sidecars() {
    assert_rejected_recovery_preserved("unsupported-journal");
}

#[test]
fn supported_recovery_replays_committed_wal_and_rolls_back_hot_journal() {
    for (mode, expected) in [
        ("supported-wal", SourceState::Failed),
        ("supported-journal", SourceState::Preparing),
    ] {
        let fixture = recovery_fixture(mode);
        let recovered = fixture.open();
        assert_eq!(recovered.source(&source().scope()).unwrap().state, expected);
        drop(recovered);
        assert_eq!(
            fixture.open().source(&source().scope()).unwrap().state,
            expected
        );
    }
}

#[test]
fn rejected_clean_wal_database_with_absent_sidecars_stays_byte_preserved() {
    use super::test_support::snapshot_recovery_files;
    let fixture = Fixture::new();
    drop(fixture.open());
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch("PRAGMA journal_mode = WAL; UPDATE schema_migrations SET version = 2;")
        .unwrap();
    drop(db);
    let before = snapshot_recovery_files(&fixture.path);
    assert!(before.iter().skip(1).all(|(_, bytes)| bytes.is_none()));
    assert_eq!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()).err(),
        Some(ArchiveError::UnsupportedSchema)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn orphan_and_ambiguous_recovery_artifacts_are_rejected_before_database_creation() {
    use super::test_support::snapshot_recovery_files;
    for suffixes in [vec!["-wal"], vec!["-journal"], vec!["-shm"]] {
        let fixture = Fixture::new();
        for suffix in suffixes {
            let path = fixture.path.with_file_name(format!("archive.db{suffix}"));
            fs::write(&path, b"synthetic orphan").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let before = snapshot_recovery_files(&fixture.path);
        assert_eq!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()).err(),
            Some(ArchiveError::InvalidStore)
        );
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
    }
    let fixture = recovery_fixture("supported-wal");
    let journal = fixture.path.with_file_name("archive.db-journal");
    fs::write(&journal, b"synthetic ambiguous journal").unwrap();
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        ArchiveStore::open(fixture.path.clone(), key(), validators()).err(),
        Some(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}
