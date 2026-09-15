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

#[path = "migration_uuid_tests.rs"]
mod uuid_tests;

pub(super) fn frozen_v1() -> (Fixture, serde_json::Value) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let encoded = include_str!("../../../test-fixtures/archive-v1/store-v1.b64");
    let manifest_bytes = include_bytes!("../../../test-fixtures/archive-v1/manifest.json");
    assert_eq!(
        Sha256::digest(manifest_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "44f9812af2f16ef8b1679f504fca16a2a99d6c31b74b67193bffd2a06c39865c"
    );
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test-fixtures/archive-v1/manifest.json"
    ))
    .unwrap();
    let bytes = STANDARD.decode(encoded.trim()).unwrap();
    assert_eq!(format!("{}\n", STANDARD.encode(&bytes)), encoded);
    assert_eq!(
        bytes.len() as u64,
        manifest["decodedBytes"].as_u64().unwrap()
    );
    assert_eq!(
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        manifest["decodedSha256"]
    );
    assert_eq!(
        manifest["creatingCommit"],
        "ae3026b92b2126d63e3ea9983eccb4f86e718108"
    );
    assert_eq!(
        manifest["schemaHash"],
        "323d5ed46b977547f54c4fae614e0804d0210ce84d6606d6a73645e8d06d9c8b"
    );
    assert!(!bytes.starts_with(b"SQLite format 3"));
    for needle in [
        "legacyneedle",
        "discordneedle",
        "example.test",
        "invented_owner",
    ] {
        assert!(!bytes.windows(needle.len()).any(|p| p == needle.as_bytes()));
    }
    let fixture = Fixture::new();
    fs::write(&fixture.path, bytes).unwrap();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o600)).unwrap();
    (fixture, manifest)
}

fn migration_validators() -> std::collections::BTreeMap<
    retract_domain::ProviderKey,
    std::sync::Arc<dyn crate::persistence::ProviderPayloadValidator>,
> {
    let mut registry = validators();
    registry.insert(
        "discord".to_owned().try_into().unwrap(),
        std::sync::Arc::new(crate::providers::discord::DiscordPayloadValidator),
    );
    registry
}

#[test]
fn frozen_v1_production_migration_preserves_all_rows_generations_and_legacy_replay() {
    let (fixture, manifest) = frozen_v1();
    let mut store =
        ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).unwrap();
    store
        .transaction(|tx| {
            assert_eq!(
                tx.query_row("SELECT version FROM schema_migrations", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            for (table, expected) in manifest["logicalRows"].as_object().unwrap() {
                let width = expected
                    .as_array()
                    .unwrap()
                    .first()
                    .map_or(0, |r| r.as_array().unwrap().len());
                let mut query = tx
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let actual = query
                    .query_map([], |row| {
                        Ok((0..width)
                            .map(|i| match row.get::<_, rusqlite::types::Value>(i).unwrap() {
                                rusqlite::types::Value::Null => serde_json::Value::Null,
                                rusqlite::types::Value::Integer(n) => serde_json::json!(n),
                                rusqlite::types::Value::Text(s) => serde_json::json!(s),
                                _ => panic!("unexpected fixture value"),
                            })
                            .collect::<Vec<_>>())
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(serde_json::json!(actual), *expected, "{table}");
            }
            for table in ["archive_import_identities", "import_warning_deltas"] {
                assert_eq!(
                    tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
            assert_eq!(
                tx.query_row(
                    "SELECT observed_at FROM import_runs WHERE provider='synthetic'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "2026-09-05T00:00:00Z"
            );
            assert_eq!(
                tx.query_row(
                    "SELECT sum(digest_version) FROM import_batch_receipts",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                2
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(super::test_support::fts_count(&store, "legacyneedle"), 1);
    assert_eq!(super::test_support::fts_count(&store, "discordneedle"), 1);
    let old = super::test_support::checkpoint(&store);
    assert_eq!(old.progress.phase, super::ImportPhase::Interrupted);
    let session = store.retry_import(&old).unwrap();
    let mut input = super::test_support::batch("frozen-v1", "legacyneedle owner@example.test");
    input.contents[0]
        .attachments
        .push(super::test_support::attachment("synthetic passport.txt"));
    let replay = store.append_batch(&session, 0, input).unwrap();
    assert_eq!(replay.committed_items, 1);
    assert_eq!(replay.committed_bytes, old.progress.committed_bytes);
    drop(store);
    let bytes = fs::read(&fixture.path).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes.windows(12).any(|p| p == b"legacyneedle"));
    drop(ArchiveStore::open(fixture.path, key(), migration_validators()).unwrap());
}

#[test]
fn frozen_v1_candidate_failures_and_unknown_newer_schema_preserve_original() {
    for mutation in [
        "UPDATE schema_migrations SET version=999",
        "DROP INDEX content_scope_order",
        "UPDATE import_runs SET committed_bytes=committed_bytes+1",
        "UPDATE actor_observations SET record_json=json_set(record_json, '$.evidence', 'live')",
    ] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        let before = snapshot_recovery_files(&fixture.path);
        assert!(ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).is_err());
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
        if mutation.contains("actor_observations") {
            let candidate_bytes = fs::read(candidate(&fixture.path)).unwrap();
            assert!(!candidate_bytes.starts_with(b"SQLite format 3"));
            assert!(
                !candidate_bytes
                    .windows(12)
                    .any(|part| part == b"legacyneedle")
            );
            let original = open_immutable_keyed(&fixture.path, &key()).unwrap();
            schema::validate_v1(&original).unwrap();
        }
    }
    let (fixture, _) = frozen_v1();
    let before = snapshot_recovery_files(&fixture.path);
    // Missing provider validation must be caught before promoting the candidate.
    assert!(ArchiveStore::open(fixture.path.clone(), key(), validators()).is_err());
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn frozen_v1_boundary_provider_input_migrates_without_recomputing_historical_findings() {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    schema::validate_v1(&db).unwrap();
    let encoded: String = db
        .query_row(
            "SELECT record_json FROM content_observations WHERE provider='discord'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut content: retract_domain::ContentRecord = model::decode(&encoded).unwrap();
    let findings = content.privacy_findings.clone();
    // Deliberately not today's detector fingerprint: recomputation must fail
    // the preservation assertion even if the resulting categories are equal.
    let detector = Some(format!("cleaner-sha256:{}", "b".repeat(64)));
    model::clear_derived_fields(&mut content);
    let mut input = model::ImportBatch {
        contents: vec![content],
        ..Default::default()
    };
    let remaining = super::MAX_BATCH_BYTES - input.bounded_size().unwrap();
    input.contents[0]
        .searchable_text
        .push_str(&"\0".repeat(remaining / 6));
    input.contents[0]
        .searchable_text
        .push_str(&"x".repeat(remaining % 6));
    assert_eq!(input.bounded_size().unwrap(), 4 * 1024 * 1024);
    input
        .validate(
            &input.contents[0].scope,
            &crate::providers::discord::DiscordPayloadValidator,
        )
        .unwrap();
    input.contents[0].searchable_text.push('x');
    assert_eq!(input.bounded_size(), Err(ArchiveError::LimitExceeded));
    input.contents[0].searchable_text.pop();
    let digest = input.digest().unwrap();
    let mut persisted = input.contents.remove(0);
    persisted.privacy_findings = findings.clone();
    persisted.detector_version = detector.clone();
    assert!(model::encode(&persisted).unwrap().len() > 4 * 1024 * 1024);
    // Synthesize a valid old ingestion result inside the authenticated frozen
    // v1 schema. Keep the historical findings/version; don't run a detector.
    let expected = model::encode(&persisted).unwrap();
    db.execute("UPDATE content_observations SET searchable_text=?1, record_json=?2 WHERE provider='discord'", rusqlite::params![persisted.searchable_text, expected]).unwrap();
    db.execute(
        "UPDATE privacy_findings SET detector_version=? WHERE provider='discord'",
        [&detector],
    )
    .unwrap();
    // Model a second legacy upsert batch before finalization, retaining the
    // first receipt that supplied this source's actor/conversation records.
    db.execute("UPDATE import_runs SET committed_bytes=committed_bytes+?1, next_batch=next_batch+1, revision=revision+1 WHERE provider='discord'", [4 * 1024 * 1024]).unwrap();
    db.execute("INSERT INTO import_batch_receipts SELECT provider,account_id,source_id,run_id,sequence+1,?2,committed_records,committed_bytes+?1,next_batch+1 FROM import_batch_receipts WHERE provider='discord'", rusqlite::params![4 * 1024 * 1024, digest]).unwrap();
    schema::validate_v1(&db).unwrap();
    drop(db);
    let store = ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).unwrap();
    store
        .transaction(|tx| {
            let actual: String = tx
                .query_row(
                    "SELECT record_json FROM content_observations WHERE provider='discord'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, expected);
            let record: retract_domain::ContentRecord = model::decode(&actual).unwrap();
            assert_eq!(record.privacy_findings, findings);
            assert_eq!(record.detector_version, detector);
            Ok(())
        })
        .unwrap();
    assert_eq!(super::test_support::fts_count(&store, "discordneedle"), 1);
    drop(store);
    let bytes = fs::read(&fixture.path).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes.windows(13).any(|part| part == b"discordneedle"));
    drop(ArchiveStore::open(fixture.path, key(), migration_validators()).unwrap());
}

fn reject_authenticated_v1_mutation(mutation: &str) {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch(mutation).unwrap();
    schema::validate_v1(&db).unwrap();
    drop(db);
    let before = snapshot_recovery_files(&fixture.path);
    assert!(
        ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).is_err(),
        "accepted inconsistent v1: {mutation}"
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    schema::validate_v1(&open_immutable_keyed(&fixture.path, &key()).unwrap()).unwrap();
    for suffix in [".migration", ".migration-wal", ".migration-journal"] {
        if let Ok(bytes) = fs::read(super::store::sidecar(&fixture.path, suffix)) {
            assert!(!bytes.starts_with(b"SQLite format 3"));
            assert!(!bytes.windows(12).any(|part| part == b"legacyneedle"));
        }
    }
}

macro_rules! inconsistent_v1 {
    ($name:ident, $sql:literal) => {
        #[test]
        fn $name() {
            reject_authenticated_v1_mutation($sql);
        }
    };
}

inconsistent_v1!(
    frozen_v1_rejects_timestamp_nanosecond_projection,
    "UPDATE content_observations SET timestamp_nanos=timestamp_nanos+1"
);
inconsistent_v1!(
    frozen_v1_rejects_search_text_projection,
    "UPDATE content_observations SET searchable_text='different synthetic text'"
);
inconsistent_v1!(
    frozen_v1_rejects_attachment_names_projection,
    "UPDATE content_observations SET attachment_names='different synthetic name'"
);
inconsistent_v1!(
    frozen_v1_rejects_conversation_reference_projection,
    "UPDATE content_observations SET conversation_id=NULL"
);
inconsistent_v1!(
    frozen_v1_rejects_actor_reference_projection,
    "UPDATE content_observations SET author_id=NULL"
);
inconsistent_v1!(
    frozen_v1_rejects_reply_reference_projection,
    "UPDATE content_observations SET reply_to_id=resource_id"
);
inconsistent_v1!(
    frozen_v1_rejects_thread_reference_projection,
    "UPDATE content_observations SET thread_parent_id=conversation_id"
);
inconsistent_v1!(
    frozen_v1_rejects_identity_locator_json,
    "UPDATE resource_identities SET locator_json=json_set(locator_json, '$.canonicalKey', 'different') WHERE provider='synthetic' AND kind='content'"
);
inconsistent_v1!(
    frozen_v1_rejects_identity_indexed_locator,
    "UPDATE resource_identities SET canonical_key='different' WHERE provider='synthetic' AND kind='content'"
);
inconsistent_v1!(
    frozen_v1_rejects_actor_catalog_mismatch,
    "UPDATE resource_identities SET locator_json=json_set(locator_json, '$.locatorPayload.nativeId', 'different') WHERE provider='synthetic' AND kind='actor'"
);
inconsistent_v1!(
    frozen_v1_rejects_conversation_catalog_mismatch,
    "UPDATE resource_identities SET locator_json=json_set(locator_json, '$.locatorPayload.nativeId', 'different') WHERE provider='synthetic' AND kind='conversation'"
);
inconsistent_v1!(
    frozen_v1_rejects_actor_scope_projection,
    "UPDATE actor_observations SET record_json=json_set(record_json, '$.scope.sourceId', '00000000-0000-0000-0000-000000000099')"
);
inconsistent_v1!(
    frozen_v1_rejects_conversation_scope_projection,
    "UPDATE conversation_observations SET record_json=json_set(record_json, '$.scope.sourceId', '00000000-0000-0000-0000-000000000099')"
);
inconsistent_v1!(
    frozen_v1_rejects_missing_ready_actor,
    "DELETE FROM actor_observations WHERE provider='discord'"
);
inconsistent_v1!(
    frozen_v1_rejects_missing_ready_conversation,
    "DELETE FROM conversation_observations WHERE provider='discord'"
);
inconsistent_v1!(
    frozen_v1_rejects_attachment_row_mismatch,
    "UPDATE attachments SET record_json=json_set(record_json, '$.sizeBytes', 6)"
);
inconsistent_v1!(
    frozen_v1_rejects_missing_attachment_row,
    "DELETE FROM attachments"
);
inconsistent_v1!(
    frozen_v1_rejects_attachment_ordinal_gap,
    "UPDATE attachments SET ordinal=1"
);
inconsistent_v1!(
    frozen_v1_rejects_extra_attachment_row,
    "INSERT INTO attachments SELECT provider, account_id, source_id, resource_id, 1, record_json FROM attachments"
);
inconsistent_v1!(
    frozen_v1_rejects_finding_row_mismatch,
    "UPDATE privacy_findings SET kind='phone_number' WHERE kind='email_address'"
);
inconsistent_v1!(
    frozen_v1_rejects_finding_version_mismatch,
    "UPDATE privacy_findings SET detector_version='historical-other-version'"
);
inconsistent_v1!(
    frozen_v1_rejects_missing_finding_row,
    "DELETE FROM privacy_findings WHERE kind='email_address'"
);
inconsistent_v1!(
    frozen_v1_rejects_extra_finding_row,
    "INSERT INTO privacy_findings SELECT provider, account_id, source_id, resource_id, 'phone_number', detector_version FROM privacy_findings WHERE kind='email_address'"
);
inconsistent_v1!(
    frozen_v1_rejects_invalid_persisted_finding_version,
    "UPDATE content_observations SET record_json=json_set(record_json, '$.detectorVersion', NULL)"
);
inconsistent_v1!(
    frozen_v1_rejects_missing_fts_postings,
    "INSERT INTO content_fts(content_fts,rowid,searchable_text,attachment_names) SELECT 'delete',observation_key,searchable_text,attachment_names FROM content_observations WHERE provider='discord'"
);
inconsistent_v1!(
    frozen_v1_rejects_extra_fts_postings,
    "INSERT INTO content_fts(rowid,searchable_text,attachment_names) VALUES(900, 'syntheticghost', '')"
);
inconsistent_v1!(
    frozen_v1_rejects_fts_document_lengths,
    "UPDATE content_fts_docsize SET sz=X'0000'"
);

#[test]
fn frozen_v1_rejects_indexed_timestamp_that_would_repeat_or_skip_cursor_results() {
    for offset in [-1, 1] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute("UPDATE content_observations SET timestamp_seconds=timestamp_seconds+? WHERE provider='discord'", [offset]).unwrap();
        let json: String = db
            .query_row(
                "SELECT record_json FROM content_observations WHERE provider='discord'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let typed: retract_domain::ContentRecord = model::decode(&json).unwrap();
        // The query orders on SQL columns but builds its next cursor from the
        // returned typed timestamp. A lower column repeats this same item on
        // that cursor; a higher column skips it below a typed-time cutoff.
        let cutoff_nanos = typed.timestamp.timestamp_subsec_nanos() + u32::from(offset > 0);
        let visible: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM content_observations WHERE provider='discord' AND (timestamp_seconds,timestamp_nanos,resource_id) < (?1,?2,?3))", rusqlite::params![typed.timestamp.timestamp(), cutoff_nanos, typed.id.as_uuid().to_string()], |row| row.get(0)).unwrap();
        assert_eq!(visible, offset < 0);
        drop(db);
        let before = snapshot_recovery_files(&fixture.path);
        assert!(
            ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).is_err(),
            "accepted timestamp offset {offset}"
        );
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
    }
}

#[test]
fn frozen_v1_interrupted_source_may_still_lack_mandatory_observations() {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch("DELETE FROM actor_observations WHERE provider='synthetic'; DELETE FROM conversation_observations WHERE provider='synthetic';").unwrap();
    drop(db);
    let store = ArchiveStore::open(fixture.path, key(), migration_validators()).unwrap();
    assert_eq!(
        super::test_support::checkpoint(&store).progress.phase,
        super::ImportPhase::Interrupted
    );
    assert_eq!(
        store.source(&source().scope()).unwrap().state,
        retract_domain::SourceState::Unavailable
    );
}

#[test]
fn frozen_v1_empty_findings_do_not_bypass_persisted_detector_version_bounds() {
    for version in [
        "x".repeat(4 * 1024 * 1024),
        "x".repeat(129),
        " ".into(),
        "bad\nversion".into(),
    ] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute("UPDATE content_observations SET record_json=json_set(record_json, '$.privacyFindings', json('[]'), '$.detectorVersion', ?) WHERE provider='discord'", [&version]).unwrap();
        db.execute("DELETE FROM privacy_findings WHERE provider='discord'", [])
            .unwrap();
        drop(db);
        let before = snapshot_recovery_files(&fixture.path);
        assert!(ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).is_err());
        assert_eq!(snapshot_recovery_files(&fixture.path), before);
    }
}

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
