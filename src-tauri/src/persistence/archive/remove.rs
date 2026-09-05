//! Exact local-source deletion and durable, content-free maintenance receipts.

use retract_domain::Scope;
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    ArchiveError, ArchiveStore, codec,
    ingest_state::{scope_sql, storage, unsigned},
    model::RemovalOutcome,
    schema, store,
};

pub(super) const PRUNE_IDENTITIES: &str = "DELETE FROM resource_identities WHERE provider=?1 AND account_id=?2 AND resource_id NOT IN (
                SELECT resource_id FROM conversation_observations WHERE provider=?1 AND account_id=?2
                UNION ALL SELECT json_extract(record_json, '$.parentId') FROM conversation_observations WHERE provider=?1 AND account_id=?2 AND json_extract(record_json, '$.parentId') IS NOT NULL
                UNION ALL SELECT resource_id FROM actor_observations WHERE provider=?1 AND account_id=?2
                UNION ALL SELECT resource_id FROM content_observations WHERE provider=?1 AND account_id=?2
                UNION ALL SELECT conversation_id FROM content_observations WHERE provider=?1 AND account_id=?2 AND conversation_id IS NOT NULL
                UNION ALL SELECT author_id FROM content_observations WHERE provider=?1 AND account_id=?2 AND author_id IS NOT NULL
                UNION ALL SELECT reply_to_id FROM content_observations WHERE provider=?1 AND account_id=?2 AND reply_to_id IS NOT NULL
                UNION ALL SELECT thread_parent_id FROM content_observations WHERE provider=?1 AND account_id=?2 AND thread_parent_id IS NOT NULL
            )";

#[derive(Clone, Copy)]
enum MaintenanceState {
    Pending,
    Completed,
}

impl MaintenanceState {
    fn parse(value: &str) -> Result<Self, ArchiveError> {
        match value {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            _ => Err(ArchiveError::InvalidStore),
        }
    }
}

impl ArchiveStore {
    pub(crate) fn remove_source(&mut self, scope: &Scope) -> Result<RemovalOutcome, ArchiveError> {
        self.transaction(|tx| {
            if receipt(tx, scope)?.is_some() || !source_exists(tx, scope)? {
                return Ok(());
            }
            codec::validate_connection_settings(tx)?;
            let s = scope_sql(scope);
            let count = tx.query_row("SELECT count(*) FROM content_observations WHERE provider=?1 AND account_id=?2 AND source_id=?3", params![s[0], s[1], s[2]], |row| unsigned(row, 0)).map_err(storage)?;
            // The receipt marks removal before deleting the source. All changes
            // commit together; no intermediate state is visible to a reader.
            tx.execute("INSERT INTO cleanup_tasks(task_id, provider, account_id, source_id, removed_items, state) VALUES(?1, ?2, ?3, ?4, ?5, 'pending')",
                params![uuid::Uuid::new_v4().to_string(), s[0], s[1], s[2], super::ingest_state::integer(count)?]).map_err(storage)?;
            // Cascades erase observations, attachments, findings, run/session,
            // receipts and warnings. FTS secure-delete triggers erase terms.
            // Deleting the run invalidates every import and query generation.
            tx.execute("DELETE FROM sources WHERE provider=?1 AND account_id=?2 AND source_id=?3", params![s[0], s[1], s[2]]).map_err(storage)?;
            // Build one memory-only membership set, instead of rescanning all
            // surviving observations for each identity. Exclude NULL so absent
            // optional references cannot prevent removal of unrelated identities.
            tx.execute(PRUNE_IDENTITIES, params![s[0], s[1]]).map_err(storage)?;
            tx.execute("DELETE FROM accounts AS a WHERE a.provider=?1 AND a.account_id=?2
                AND NOT EXISTS(SELECT 1 FROM sources s WHERE s.provider=a.provider AND s.account_id=a.account_id)
                AND NOT EXISTS(SELECT 1 FROM resource_identities r WHERE r.provider=a.provider AND r.account_id=a.account_id)", params![s[0], s[1]]).map_err(storage)?;
            Ok(())
        })?;
        self.retry_cleanup(scope)
    }

    pub(crate) fn retry_cleanup(&mut self, scope: &Scope) -> Result<RemovalOutcome, ArchiveError> {
        // The sole connection and all read statements are drained under this
        // worker mutex. No public query returns a live SQLite reader.
        let connection = self
            .connection
            .lock()
            .map_err(|_| ArchiveError::StorageFailure)?;
        let Some(mut outcome) = receipt(&connection, scope)? else {
            return if source_exists(&connection, scope)? {
                Err(ArchiveError::InvalidRecord)
            } else {
                Ok(RemovalOutcome {
                    removed_items: 0,
                    maintenance_pending: false,
                })
            };
        };
        if !outcome.maintenance_pending {
            return Ok(outcome);
        }
        let maintain = || {
            store::validate_artifacts(&self.path)?;
            #[cfg(test)]
            if let Some(hook) = &self.before_maintenance {
                hook(&connection)?;
            }
            compact(&connection)?;
            store::validate_artifacts(&self.path)?;
            // Completion is recorded only after compaction and verification.
            // If this commit fails, the durable receipt remains pending.
            let s = scope_sql(scope);
            connection.execute("UPDATE cleanup_tasks SET state='completed' WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND state='pending'", params![s[0], s[1], s[2]]).map_err(storage)?;
            Ok::<_, ArchiveError>(())
        };
        if maintain().is_ok() {
            outcome.maintenance_pending = false;
        }
        Ok(outcome)
    }
}

fn compact(connection: &Connection) -> Result<(), ArchiveError> {
    codec::validate_connection_settings(connection)?;
    checkpoint(connection)?;
    // MEMORY is mandatory: ordinary VACUUM can require substantial RAM and
    // additional disk. Resource/I/O failure leaves logical deletion committed
    // and maintenance pending. Never fall back to file-based temporary stores.
    connection.execute_batch("VACUUM").map_err(storage)?;
    checkpoint(connection)?;
    codec::validate_connection_settings(connection)?;
    schema::validate(connection)?;
    connection
        .execute(
            "INSERT INTO content_fts(content_fts, rank) VALUES('integrity-check', 1)",
            [],
        )
        .map_err(storage)?;
    let free: i64 = connection
        .pragma_query_value(None, "freelist_count", |row| row.get(0))
        .map_err(storage)?;
    if free != 0 {
        return Err(ArchiveError::CleanupPending);
    }
    Ok(())
}

pub(super) fn checkpoint(connection: &Connection) -> Result<(), ArchiveError> {
    let (busy, log, checkpointed): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage)?;
    if busy != 0 || log != checkpointed {
        return Err(ArchiveError::CleanupPending);
    }
    Ok(())
}

fn source_exists(connection: &Connection, scope: &Scope) -> Result<bool, ArchiveError> {
    let owner: Option<(String, String)> = connection
        .query_row(
            "SELECT provider, account_id FROM sources WHERE source_id=?",
            [scope.source_id.as_uuid().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    match owner {
        Some((provider, account))
            if provider != scope.provider.as_str()
                || account != scope.account_id.as_uuid().to_string() =>
        {
            Err(ArchiveError::ScopeMismatch)
        }
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

fn receipt(connection: &Connection, scope: &Scope) -> Result<Option<RemovalOutcome>, ArchiveError> {
    let row: Option<(String, String, u64, String)> = connection.query_row("SELECT provider, account_id, removed_items, state FROM cleanup_tasks WHERE source_id=?", [scope.source_id.as_uuid().to_string()], |row| Ok((row.get(0)?, row.get(1)?, unsigned(row, 2)?, row.get(3)?))).optional().map_err(storage)?;
    let Some((provider, account, removed_items, state)) = row else {
        return Ok(None);
    };
    if provider != scope.provider.as_str() || account != scope.account_id.as_uuid().to_string() {
        return Err(ArchiveError::ScopeMismatch);
    }
    Ok(Some(RemovalOutcome {
        removed_items,
        maintenance_pending: matches!(MaintenanceState::parse(&state)?, MaintenanceState::Pending),
    }))
}
