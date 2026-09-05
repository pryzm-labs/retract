//! Persisted import runs, exact provenance, and backend-only retry authority.

use chrono::Utc;
use retract_domain::{Scope, SourceRecord, SourceState, VersionedPayload};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use super::{
    ArchiveError, ArchiveStore,
    model::{
        self, ImportCheckpoint, ImportLimits, ImportPhase, ImportProgress, ImportSession,
        ImportWarning,
    },
};

pub(super) struct Run {
    pub(super) checkpoint: ImportCheckpoint,
    pub(super) session_id: Uuid,
    pub(super) policy: String,
}

impl ArchiveStore {
    pub(crate) fn begin_import(&mut self, scope: &Scope) -> Result<ImportSession, ArchiveError> {
        let validator = self
            .validators
            .get(&scope.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        self.transaction(|tx| {
            let source = read_source(tx, scope)?;
            if source.state != SourceState::Preparing || load_run(tx, scope)?.is_some() { return Err(ArchiveError::StaleCursor); }
            let session = ImportSession { id: Uuid::new_v4(), scope: scope.clone(), fingerprint: source.archive_fingerprint.ok_or(ArchiveError::InvalidRecord)?, schema_profile: source.schema_profile, cancellation: Default::default() };
            let s = scope_sql(scope);
            tx.execute("INSERT INTO import_runs(provider, account_id, source_id, run_id, session_id, fingerprint, schema_profile, validation_policy, state) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'importing')",
                params![s[0], s[1], s[2], Uuid::new_v4().to_string(), session.id.to_string(), session.fingerprint, model::encode(&session.schema_profile)?, validator.validation_policy_key().as_str()]).map_err(storage)?;
            Ok(session)
        })
    }

    pub(crate) fn import_status(
        &self,
        scope: &Scope,
        fingerprint: &str,
        schema_profile: &VersionedPayload,
    ) -> Result<Option<ImportCheckpoint>, ArchiveError> {
        self.transaction(|tx| {
            check_provenance(&read_source(tx, scope)?, fingerprint, schema_profile)?;
            let Some(run) = load_run(tx, scope)? else {
                return Ok(None);
            };
            self.validate_run(tx, &run)?;
            Ok(Some(run.checkpoint))
        })
    }

    pub(crate) fn retry_import(
        &mut self,
        expected: &ImportCheckpoint,
    ) -> Result<ImportSession, ArchiveError> {
        self.transaction(|tx| {
            let mut run = load_run(tx, &expected.scope)?.ok_or(ArchiveError::StaleCursor)?;
            self.validate_run(tx, &run)?;
            if run.checkpoint != *expected
                || !matches!(
                    expected.progress.phase,
                    ImportPhase::Interrupted | ImportPhase::Cancelled | ImportPhase::Failed
                )
            {
                return Err(ArchiveError::StaleCursor);
            }
            validate_checkpoint_integrity(tx, &run)?;
            run.session_id = Uuid::new_v4();
            run.checkpoint.progress.phase = ImportPhase::Importing;
            save_run(tx, &mut run)?;
            set_source_phase(tx, &run.checkpoint.scope, ImportPhase::Importing)?;
            Ok(ImportSession {
                id: run.session_id,
                scope: expected.scope.clone(),
                fingerprint: expected.fingerprint.clone(),
                schema_profile: expected.schema_profile.clone(),
                cancellation: Default::default(),
            })
        })
    }

    pub(crate) fn cancel_import(
        &mut self,
        session: &ImportSession,
    ) -> Result<ImportProgress, ArchiveError> {
        let validator = self
            .validators
            .get(&session.scope.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        self.transaction(|tx| {
            let mut run = active_session(
                tx,
                session,
                validator.validation_policy_key().as_str(),
                self.import_limits,
            )?;
            run.checkpoint.progress.phase = ImportPhase::Cancelled;
            save_run(tx, &mut run)?;
            set_source_phase(tx, &session.scope, ImportPhase::Cancelled)?;
            Ok(run.checkpoint.progress)
        })
    }

    /// Query consumers use this same guard inside their read transaction.
    pub(crate) fn require_ready(&self, scope: &Scope) -> Result<(), ArchiveError> {
        self.transaction(|tx| require_ready(tx, scope))
    }

    fn validate_run(&self, connection: &Connection, run: &Run) -> Result<(), ArchiveError> {
        let validator = self
            .validators
            .get(&run.checkpoint.scope.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        validate_run(
            connection,
            run,
            validator.validation_policy_key().as_str(),
            self.import_limits,
        )
    }
}

pub(super) fn require_ready(connection: &Connection, scope: &Scope) -> Result<(), ArchiveError> {
    if read_source(connection, scope)?.state != SourceState::Ready
        || load_run(connection, scope)?
            .is_none_or(|run| run.checkpoint.progress.phase != ImportPhase::Ready)
    {
        return Err(ArchiveError::IncompleteSource);
    }
    Ok(())
}

pub(super) fn active_session(
    connection: &Connection,
    session: &ImportSession,
    policy: &str,
    limits: ImportLimits,
) -> Result<Run, ArchiveError> {
    check_provenance(
        &read_source(connection, &session.scope)?,
        &session.fingerprint,
        &session.schema_profile,
    )?;
    let run = load_run(connection, &session.scope)?.ok_or(ArchiveError::StaleCursor)?;
    validate_run(connection, &run, policy, limits)?;
    if run.session_id != session.id {
        return Err(ArchiveError::StaleCursor);
    }
    if run.checkpoint.progress.phase == ImportPhase::Cancelled {
        return Err(ArchiveError::Cancelled);
    }
    if run.checkpoint.progress.phase != ImportPhase::Importing {
        return Err(ArchiveError::StaleCursor);
    }
    Ok(run)
}

pub(super) fn validate_run(
    connection: &Connection,
    run: &Run,
    policy: &str,
    limits: ImportLimits,
) -> Result<(), ArchiveError> {
    let checkpoint = &run.checkpoint;
    check_provenance(
        &read_source(connection, &checkpoint.scope)?,
        &checkpoint.fingerprint,
        &checkpoint.schema_profile,
    )?;
    if run.policy != policy {
        return Err(ArchiveError::InvalidRecord);
    }
    if checkpoint.progress.committed_items > limits.items
        || checkpoint.progress.committed_bytes > limits.bytes
    {
        return Err(ArchiveError::LimitExceeded);
    }
    Ok(())
}

/// Full, read-only validation also runs on the encrypted preflight copy. No
/// original is marked interrupted until every source/run has passed it.
pub(super) fn validate_runs(
    connection: &Connection,
    validators: &std::collections::BTreeMap<
        retract_domain::ProviderKey,
        std::sync::Arc<dyn crate::persistence::ProviderPayloadValidator>,
    >,
) -> Result<(), ArchiveError> {
    let mut query = connection.prepare("SELECT s.record_json FROM sources s JOIN import_runs r USING(provider, account_id, source_id)").map_err(storage)?;
    let mut rows = query.query([]).map_err(storage)?;
    while let Some(row) = rows.next().map_err(storage)? {
        let source: SourceRecord = model::decode(&row.get::<_, String>(0).map_err(storage)?)?;
        let run = load_run(connection, &source.scope())?.ok_or(ArchiveError::InvalidStore)?;
        let validator = validators
            .get(&source.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        validate_run(
            connection,
            &run,
            validator.validation_policy_key().as_str(),
            ImportLimits::default(),
        )?;
        validate_checkpoint_integrity(connection, &run)?;
        let expected_state = match run.checkpoint.progress.phase {
            ImportPhase::Importing => SourceState::Preparing,
            ImportPhase::Ready => SourceState::Ready,
            ImportPhase::Failed => SourceState::Failed,
            ImportPhase::Interrupted | ImportPhase::Cancelled => SourceState::Unavailable,
        };
        if source.state != expected_state {
            return Err(ArchiveError::InvalidStore);
        }
    }
    Ok(())
}

pub(super) fn interrupt_runs(connection: &mut Connection) -> Result<(), ArchiveError> {
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage)?;
    {
        let mut query = tx.prepare("SELECT s.record_json FROM sources s JOIN import_runs r USING(provider, account_id, source_id) WHERE r.state='importing'").map_err(storage)?;
        let mut rows = query.query([]).map_err(storage)?;
        while let Some(row) = rows.next().map_err(storage)? {
            let source: SourceRecord = model::decode(&row.get::<_, String>(0).map_err(storage)?)?;
            let mut run = load_run(&tx, &source.scope())?.ok_or(ArchiveError::InvalidStore)?;
            run.checkpoint.progress.phase = ImportPhase::Interrupted;
            save_run(&tx, &mut run)?;
            set_source_phase(&tx, &source.scope(), ImportPhase::Interrupted)?;
        }
    }
    tx.commit().map_err(storage)
}

fn validate_checkpoint_integrity(connection: &Connection, run: &Run) -> Result<(), ArchiveError> {
    let s = scope_sql(&run.checkpoint.scope);
    let items = connection.query_row("SELECT count(*) FROM content_observations WHERE provider=?1 AND account_id=?2 AND source_id=?3", params![s[0], s[1], s[2]], |row| unsigned(row, 0)).map_err(storage)?;
    if items != run.checkpoint.progress.committed_items {
        return Err(ArchiveError::InvalidStore);
    }
    let mut query = connection.prepare("SELECT sequence, digest, committed_records, committed_bytes, next_batch FROM import_batch_receipts WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4 ORDER BY sequence").map_err(storage)?;
    let mut rows = query
        .query(params![s[0], s[1], s[2], run.checkpoint.run_id.to_string()])
        .map_err(storage)?;
    let (mut sequence, mut items, mut bytes) = (0_u64, 0_u64, 0_u64);
    while let Some(row) = rows.next().map_err(storage)? {
        let next_items = unsigned(row, 2).map_err(storage)?;
        let next_bytes = unsigned(row, 3).map_err(storage)?;
        let digest: String = row.get(1).map_err(storage)?;
        if unsigned(row, 0).map_err(storage)? != sequence
            || unsigned(row, 4).map_err(storage)?
                != sequence.checked_add(1).ok_or(ArchiveError::InvalidStore)?
            || next_items < items
            || next_items > run.checkpoint.progress.committed_items
            || next_bytes <= bytes
            || next_bytes - bytes > model::MAX_BATCH_BYTES as u64
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ArchiveError::InvalidStore);
        }
        sequence += 1;
        items = next_items;
        bytes = next_bytes;
    }
    let progress = &run.checkpoint.progress;
    if sequence != progress.next_batch
        || items != progress.committed_items
        || bytes != progress.committed_bytes
    {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

pub(super) fn check_provenance(
    source: &SourceRecord,
    fingerprint: &str,
    schema: &VersionedPayload,
) -> Result<(), ArchiveError> {
    if source.archive_fingerprint.as_deref() != Some(fingerprint)
        || source.schema_profile != *schema
    {
        return Err(ArchiveError::StaleCursor);
    }
    Ok(())
}

pub(super) fn read_source(
    connection: &Connection,
    scope: &Scope,
) -> Result<SourceRecord, ArchiveError> {
    let s = scope_sql(scope);
    let json = connection
        .query_row(
            "SELECT record_json FROM sources WHERE provider=?1 AND account_id=?2 AND source_id=?3",
            params![s[0], s[1], s[2]],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or(ArchiveError::ScopeMismatch)?;
    let source: SourceRecord = model::decode(&json)?;
    if source.scope() != *scope {
        return Err(ArchiveError::ScopeMismatch);
    }
    Ok(source)
}

pub(super) fn load_run(
    connection: &Connection,
    scope: &Scope,
) -> Result<Option<Run>, ArchiveError> {
    let s = scope_sql(scope);
    let raw = connection.query_row("SELECT run_id, session_id, fingerprint, schema_profile, validation_policy, revision, state, committed_records, committed_bytes, next_batch FROM import_runs WHERE provider=?1 AND account_id=?2 AND source_id=?3", params![s[0], s[1], s[2]], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, unsigned(row, 5)?, row.get::<_, String>(6)?, unsigned(row, 7)?, unsigned(row, 8)?, unsigned(row, 9)?))
    }).optional().map_err(storage)?;
    let Some((
        run_id,
        session_id,
        fingerprint,
        schema,
        policy,
        revision,
        state,
        items,
        bytes,
        next_batch,
    )) = raw
    else {
        return Ok(None);
    };
    let mut query = connection.prepare("SELECT code, count FROM import_warnings WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4 ORDER BY code LIMIT 33").map_err(storage)?;
    let warnings = query
        .query_map(params![s[0], s[1], s[2], run_id], |row| {
            Ok(ImportWarning {
                code: row.get(0)?,
                count: unsigned(row, 1)?,
            })
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    if warnings.len() > model::MAX_WARNING_CODES
        || warnings.iter().any(|warning| {
            !matches!(warning.code.as_str(), "missing_reply" | "missing_thread")
                || warning.count == 0
        })
    {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(Some(Run {
        session_id: uuid(&session_id)?,
        policy,
        checkpoint: ImportCheckpoint {
            scope: scope.clone(),
            fingerprint,
            schema_profile: model::decode(&schema)?,
            run_id: uuid(&run_id)?,
            revision,
            progress: ImportProgress {
                phase: parse_phase(&state)?,
                committed_items: items,
                committed_bytes: bytes,
                next_batch,
            },
            warnings,
        },
    }))
}

pub(super) fn save_run(tx: &Transaction<'_>, run: &mut Run) -> Result<(), ArchiveError> {
    let s = scope_sql(&run.checkpoint.scope);
    run.checkpoint.revision = run
        .checkpoint
        .revision
        .checked_add(1)
        .ok_or(ArchiveError::LimitExceeded)?;
    let p = &run.checkpoint.progress;
    tx.execute("UPDATE import_runs SET state=?5, committed_records=?6, committed_bytes=?7, next_batch=?8, revision=?9, session_id=?10 WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4",
        params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), phase_name(p.phase), integer(p.committed_items)?, integer(p.committed_bytes)?, integer(p.next_batch)?, integer(run.checkpoint.revision)?, run.session_id.to_string()]).map_err(storage)?;
    Ok(())
}

pub(super) fn set_source_phase(
    tx: &Transaction<'_>,
    scope: &Scope,
    phase: ImportPhase,
) -> Result<(), ArchiveError> {
    let mut source = read_source(tx, scope)?;
    source.state = match phase {
        ImportPhase::Ready => SourceState::Ready,
        ImportPhase::Importing => SourceState::Preparing,
        ImportPhase::Failed => SourceState::Failed,
        _ => SourceState::Unavailable,
    };
    source.updated_at = Utc::now();
    source.imported_at = (phase == ImportPhase::Ready).then_some(source.updated_at);
    let s = scope_sql(scope);
    tx.execute(
        "UPDATE sources SET record_json=?4 WHERE provider=?1 AND account_id=?2 AND source_id=?3",
        params![s[0], s[1], s[2], model::encode(&source)?],
    )
    .map_err(storage)?;
    Ok(())
}

pub(super) fn scope_sql(scope: &Scope) -> [String; 3] {
    [
        scope.provider.as_str().to_owned(),
        scope.account_id.as_uuid().to_string(),
        scope.source_id.as_uuid().to_string(),
    ]
}
pub(super) fn integer(value: u64) -> Result<i64, ArchiveError> {
    i64::try_from(value).map_err(|_| ArchiveError::LimitExceeded)
}
pub(super) fn unsigned(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}
pub(super) fn uuid(value: &str) -> Result<Uuid, ArchiveError> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or(ArchiveError::InvalidStore)
}
pub(super) fn storage(_: rusqlite::Error) -> ArchiveError {
    ArchiveError::StorageFailure
}
pub(super) fn phase_name(phase: ImportPhase) -> &'static str {
    match phase {
        ImportPhase::Importing => "importing",
        ImportPhase::Ready => "ready",
        ImportPhase::Interrupted => "interrupted",
        ImportPhase::Cancelled => "cancelled",
        ImportPhase::Failed => "failed",
    }
}
pub(super) fn parse_phase(phase: &str) -> Result<ImportPhase, ArchiveError> {
    match phase {
        "importing" => Ok(ImportPhase::Importing),
        "ready" => Ok(ImportPhase::Ready),
        "interrupted" => Ok(ImportPhase::Interrupted),
        "cancelled" => Ok(ImportPhase::Cancelled),
        "failed" => Ok(ImportPhase::Failed),
        _ => Err(ArchiveError::InvalidStore),
    }
}
