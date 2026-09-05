//! Authenticated archive schema, independent of live jobs and sessions.

use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::ArchiveError;

const VERSION: i64 = 1;
const APPLICATION: &str = "retract.archive-index";
// Intentional DDL edits require an explicit reviewed fingerprint update.
const EXPECTED_SCHEMA_HASH: &str =
    "180bad4ac87c6b682347c94689969cb6aad6179985f781ae13f2482005aaac1f";

const TABLES: &str = "
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    application TEXT NOT NULL,
    schema_hash TEXT NOT NULL
) STRICT;
CREATE TABLE accounts (
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL UNIQUE,
    canonical_identity TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(provider, account_id),
    UNIQUE(provider, canonical_identity)
) STRICT;
CREATE TABLE sources (
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL,
    source_id TEXT NOT NULL UNIQUE,
    record_json TEXT NOT NULL,
    PRIMARY KEY(provider, account_id, source_id),
    FOREIGN KEY(provider, account_id) REFERENCES accounts(provider, account_id)
) STRICT;
CREATE TABLE resource_identities (
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL,
    resource_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind IN ('conversation', 'actor', 'content', 'grouping')),
    locator_schema TEXT NOT NULL,
    locator_version INTEGER NOT NULL CHECK(locator_version > 0),
    canonical_key TEXT NOT NULL,
    locator_json TEXT NOT NULL,
    PRIMARY KEY(provider, account_id, resource_id),
    UNIQUE(provider, account_id, resource_id, kind),
    UNIQUE(provider, account_id, kind, locator_schema, locator_version, canonical_key),
    FOREIGN KEY(provider, account_id) REFERENCES accounts(provider, account_id)
) STRICT;
CREATE TABLE conversation_observations (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    resource_id TEXT NOT NULL, record_json TEXT NOT NULL,
    resource_kind TEXT NOT NULL DEFAULT 'conversation' CHECK(resource_kind = 'conversation'),
    PRIMARY KEY(provider, account_id, source_id, resource_id),
    FOREIGN KEY(provider, account_id, source_id) REFERENCES sources(provider, account_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY(provider, account_id, resource_id, resource_kind) REFERENCES resource_identities(provider, account_id, resource_id, kind)
) STRICT;
CREATE TABLE actor_observations (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    resource_id TEXT NOT NULL, record_json TEXT NOT NULL,
    resource_kind TEXT NOT NULL DEFAULT 'actor' CHECK(resource_kind = 'actor'),
    PRIMARY KEY(provider, account_id, source_id, resource_id),
    FOREIGN KEY(provider, account_id, source_id) REFERENCES sources(provider, account_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY(provider, account_id, resource_id, resource_kind) REFERENCES resource_identities(provider, account_id, resource_id, kind)
) STRICT;
CREATE TABLE content_observations (
    observation_key INTEGER PRIMARY KEY,
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    resource_id TEXT NOT NULL, conversation_id TEXT, author_id TEXT, reply_to_id TEXT, thread_parent_id TEXT,
    timestamp_seconds INTEGER, timestamp_nanos INTEGER CHECK(timestamp_nanos >= 0 AND timestamp_nanos <= 1999999999),
    searchable_text TEXT NOT NULL, attachment_names TEXT NOT NULL DEFAULT '', record_json TEXT NOT NULL,
    resource_kind TEXT NOT NULL DEFAULT 'content' CHECK(resource_kind = 'content'),
    UNIQUE(provider, account_id, source_id, resource_id),
    FOREIGN KEY(provider, account_id, source_id) REFERENCES sources(provider, account_id, source_id) ON DELETE CASCADE,
    FOREIGN KEY(provider, account_id, resource_id, resource_kind) REFERENCES resource_identities(provider, account_id, resource_id, kind)
) STRICT;
CREATE INDEX content_scope_order ON content_observations(provider, account_id, source_id, timestamp_seconds, timestamp_nanos, resource_id);
CREATE INDEX content_conversation_reference ON content_observations(conversation_id);
CREATE INDEX content_author_reference ON content_observations(author_id);
CREATE INDEX content_reply_reference ON content_observations(reply_to_id);
CREATE INDEX content_thread_reference ON content_observations(thread_parent_id);
CREATE TABLE attachments (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    resource_id TEXT NOT NULL, ordinal INTEGER NOT NULL CHECK(ordinal >= 0), record_json TEXT NOT NULL,
    PRIMARY KEY(provider, account_id, source_id, resource_id, ordinal),
    FOREIGN KEY(provider, account_id, source_id, resource_id) REFERENCES content_observations(provider, account_id, source_id, resource_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE privacy_findings (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    resource_id TEXT NOT NULL, kind TEXT NOT NULL, detector_version TEXT NOT NULL,
    PRIMARY KEY(provider, account_id, source_id, resource_id, kind),
    FOREIGN KEY(provider, account_id, source_id, resource_id) REFERENCES content_observations(provider, account_id, source_id, resource_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX privacy_scope_kind ON privacy_findings(provider, account_id, source_id, kind, resource_id);
CREATE TABLE import_runs (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    run_id TEXT NOT NULL UNIQUE, session_id TEXT NOT NULL UNIQUE,
    fingerprint TEXT NOT NULL, schema_profile TEXT NOT NULL, validation_policy TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
    state TEXT NOT NULL CHECK(state IN ('importing', 'ready', 'interrupted', 'cancelled', 'failed')),
    committed_records INTEGER NOT NULL DEFAULT 0 CHECK(committed_records >= 0),
    committed_bytes INTEGER NOT NULL DEFAULT 0 CHECK(committed_bytes >= 0),
    next_batch INTEGER NOT NULL DEFAULT 0 CHECK(next_batch >= 0),
    PRIMARY KEY(provider, account_id, source_id, run_id),
    UNIQUE(provider, account_id, source_id),
    FOREIGN KEY(provider, account_id, source_id) REFERENCES sources(provider, account_id, source_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE import_batch_receipts (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    run_id TEXT NOT NULL, sequence INTEGER NOT NULL CHECK(sequence >= 0), digest TEXT NOT NULL,
    committed_records INTEGER NOT NULL CHECK(committed_records >= 0),
    committed_bytes INTEGER NOT NULL CHECK(committed_bytes >= 0),
    next_batch INTEGER NOT NULL CHECK(next_batch > 0),
    PRIMARY KEY(provider, account_id, source_id, run_id, sequence),
    FOREIGN KEY(provider, account_id, source_id, run_id) REFERENCES import_runs(provider, account_id, source_id, run_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE import_warnings (
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    run_id TEXT NOT NULL, code TEXT NOT NULL, count INTEGER NOT NULL CHECK(count > 0),
    PRIMARY KEY(provider, account_id, source_id, run_id, code),
    FOREIGN KEY(provider, account_id, source_id, run_id) REFERENCES import_runs(provider, account_id, source_id, run_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE cleanup_tasks (
    task_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL, account_id TEXT NOT NULL, source_id TEXT NOT NULL,
    state TEXT NOT NULL
) STRICT;
-- Cleanup provenance must be established while the source exists, then survive
-- its logical deletion so later compaction can update the same tombstone.
CREATE TRIGGER cleanup_scope_insert BEFORE INSERT ON cleanup_tasks BEGIN
    SELECT RAISE(ABORT, 'archive cleanup scope') WHERE NOT EXISTS (
        SELECT 1 FROM sources s WHERE s.provider = new.provider
        AND s.account_id = new.account_id AND s.source_id = new.source_id
    );
END;
CREATE TRIGGER cleanup_scope_update BEFORE UPDATE ON cleanup_tasks
WHEN old.provider != new.provider OR old.account_id != new.account_id OR old.source_id != new.source_id BEGIN
    SELECT RAISE(ABORT, 'archive cleanup scope');
END;
CREATE VIRTUAL TABLE content_fts USING fts5(searchable_text, attachment_names, content='content_observations', content_rowid='observation_key');
CREATE TRIGGER content_fts_insert AFTER INSERT ON content_observations BEGIN
    INSERT INTO content_fts(rowid, searchable_text, attachment_names) VALUES(new.observation_key, new.searchable_text, new.attachment_names);
END;
CREATE TRIGGER content_fts_delete AFTER DELETE ON content_observations BEGIN
    INSERT INTO content_fts(content_fts, rowid, searchable_text, attachment_names) VALUES('delete', old.observation_key, old.searchable_text, old.attachment_names);
END;
CREATE TRIGGER content_fts_update AFTER UPDATE ON content_observations BEGIN
    INSERT INTO content_fts(content_fts, rowid, searchable_text, attachment_names) VALUES('delete', old.observation_key, old.searchable_text, old.attachment_names);
    INSERT INTO content_fts(rowid, searchable_text, attachment_names) VALUES(new.observation_key, new.searchable_text, new.attachment_names);
END;
";

pub(super) fn initialize(connection: &mut Connection) -> Result<(), ArchiveError> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| ArchiveError::StorageFailure)?;
    tx.execute_batch(TABLES)
        .map_err(|_| ArchiveError::StorageFailure)?;
    // References may arrive before their observations. Reject known foreign owners
    // both when adding the reference and when resolving an earlier unknown UUID.
    for operation in ["INSERT", "UPDATE"] {
        let suffix = operation.to_ascii_lowercase();
        tx.execute_batch(&format!("
CREATE TRIGGER content_reference_scope_{suffix} BEFORE {operation} ON content_observations BEGIN
    SELECT RAISE(ABORT, 'archive reference scope') WHERE EXISTS (
        SELECT 1 FROM resource_identities r
        WHERE r.resource_id IN (new.conversation_id, new.author_id, new.reply_to_id, new.thread_parent_id)
        AND (r.provider != new.provider OR r.account_id != new.account_id)
    );
END;
CREATE TRIGGER resource_reference_scope_{suffix} BEFORE {operation} ON resource_identities BEGIN
    SELECT RAISE(ABORT, 'archive reference scope') WHERE EXISTS (
        SELECT 1 FROM content_observations c
        WHERE (new.resource_id = c.conversation_id OR new.resource_id = c.author_id
            OR new.resource_id = c.reply_to_id OR new.resource_id = c.thread_parent_id)
        AND (c.provider != new.provider OR c.account_id != new.account_id)
    );
END;
")).map_err(|_| ArchiveError::StorageFailure)?;
    }
    tx.execute(
        "INSERT INTO content_fts(content_fts, rank) VALUES('secure-delete', 1)",
        [],
    )
    .map_err(|_| ArchiveError::UnsupportedCodec)?;
    let hash = schema_hash(&tx)?;
    #[cfg(test)]
    assert_eq!(
        hash, EXPECTED_SCHEMA_HASH,
        "intentional DDL changes require reviewed fingerprint updates"
    );
    if hash != EXPECTED_SCHEMA_HASH {
        return Err(ArchiveError::UnsupportedSchema);
    }
    tx.execute(
        "INSERT INTO schema_migrations(version, application, schema_hash) VALUES(?, ?, ?)",
        (VERSION, APPLICATION, hash),
    )
    .map_err(|_| ArchiveError::StorageFailure)?;
    tx.commit().map_err(|_| ArchiveError::StorageFailure)
}

pub(super) fn validate(connection: &Connection) -> Result<(), ArchiveError> {
    let mut query = connection
        .prepare("SELECT version, application, schema_hash FROM schema_migrations")
        .map_err(|_| ArchiveError::UnsupportedSchema)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| ArchiveError::UnsupportedSchema)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ArchiveError::InvalidStore)?;
    if rows.len() != 1
        || rows[0].0 != VERSION
        || rows[0].1 != APPLICATION
        || rows[0].2 != EXPECTED_SCHEMA_HASH
        || rows[0].2 != schema_hash(connection)?
    {
        return Err(ArchiveError::UnsupportedSchema);
    }
    let secure_delete: i64 = connection
        .query_row(
            "SELECT v FROM content_fts_config WHERE k = 'secure-delete'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| ArchiveError::InvalidStore)?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| ArchiveError::InvalidStore)?;
    let foreign_errors = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|_| ArchiveError::InvalidStore)?
        .exists([])
        .map_err(|_| ArchiveError::InvalidStore)?;
    if secure_delete != 1 || integrity != "ok" || foreign_errors {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

fn schema_hash(connection: &Connection) -> Result<String, ArchiveError> {
    let mut query = connection
        .prepare(
            "SELECT type, name, tbl_name, coalesce(sql, '') FROM sqlite_schema ORDER BY type, name",
        )
        .map_err(|_| ArchiveError::InvalidStore)?;
    let mut rows = query.query([]).map_err(|_| ArchiveError::InvalidStore)?;
    let mut digest = Sha256::new();
    while let Some(row) = rows.next().map_err(|_| ArchiveError::InvalidStore)? {
        for column in 0..4 {
            let value: String = row.get(column).map_err(|_| ArchiveError::InvalidStore)?;
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn initial_schema_matches_the_binary_fingerprint() {
        let fixture = super::super::test_support::Fixture::new();
        let mut connection = super::super::codec::open_keyed(
            &fixture.path,
            &super::super::test_support::key(),
            true,
        )
        .unwrap();
        // Initialization asserts the computed hash on an intentional DDL edit.
        super::initialize(&mut connection).unwrap();
        let hash = super::schema_hash(&connection).unwrap();
        assert_eq!(hash, super::EXPECTED_SCHEMA_HASH);
    }
}
