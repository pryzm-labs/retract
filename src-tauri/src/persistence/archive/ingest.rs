//! Backend-only transactional archive ingestion. Authority and checkpoints are
//! checked in the same transaction as their dependent writes.

use std::collections::BTreeSet;

use retract_domain::{ProviderResourceRef, ResourceKind, Scope};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use super::{
    ArchiveError, ArchiveStore,
    ingest_state::{
        Run, active_session, integer, save_run, scope_sql, set_source_phase, storage, unsigned,
    },
    model::{self, ImportBatch, ImportPhase, ImportProgress, ImportSession},
};

impl ArchiveStore {
    pub(crate) fn append_batch(
        &mut self,
        session: &ImportSession,
        sequence: u64,
        batch: ImportBatch,
    ) -> Result<ImportProgress, ArchiveError> {
        self.append(session, sequence, batch, None)
    }

    pub(crate) fn append_batch_v2(
        &mut self,
        session: &ImportSession,
        sequence: u64,
        mut batch: model::ImportBatchV2,
    ) -> Result<ImportProgress, ArchiveError> {
        batch.bounded_size()?;
        batch.normalize_provider_fields();
        self.append(session, sequence, batch.records, Some(batch.warnings))
    }

    fn append(
        &mut self,
        session: &ImportSession,
        sequence: u64,
        mut batch: ImportBatch,
        warnings: Option<Vec<model::ImportWarningDelta>>,
    ) -> Result<ImportProgress, ArchiveError> {
        session.cancellation.check()?;
        let (bytes, digest, version) = if let Some(warnings) = &warnings {
            let payload = model::batch_v2_payload(&batch, warnings);
            (
                model::encoded_size(&payload, model::MAX_BATCH_BYTES)? as u64,
                model::digest_serialized(&payload)?,
                2,
            )
        } else {
            (batch.bounded_size()? as u64, batch.digest()?, 1)
        };
        let validator = self
            .validators
            .get(&session.scope.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        self.transaction_checked(|tx| {
            let mut run = active_session(tx, session, validator.validation_policy_key().as_str(), self.import_limits)?;
            let s = scope_sql(&session.scope);
            let progress = &run.checkpoint.progress;
            if sequence < progress.next_batch {
                let receipt = tx.query_row("SELECT digest, committed_records, committed_bytes, next_batch, digest_version FROM import_batch_receipts WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4 AND sequence=?5",
                    params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), integer(sequence)?], |row| {
                        Ok((row.get::<_, String>(0)?, ImportProgress { phase: ImportPhase::Importing, committed_items: unsigned(row, 1)?, committed_bytes: unsigned(row, 2)?, next_batch: unsigned(row, 3)? }, row.get::<_, i64>(4)?))
                    }).optional().map_err(storage)?.ok_or(ArchiveError::InvalidStore)?;
                return if receipt.0 == digest && receipt.2 == version { Ok(receipt.1) } else { Err(ArchiveError::StaleCursor) };
            }
            if sequence != progress.next_batch { return Err(ArchiveError::StaleCursor); }
            let committed_bytes = progress.committed_bytes.checked_add(bytes).ok_or(ArchiveError::LimitExceeded)?;
            if committed_bytes > self.import_limits.bytes { return Err(ArchiveError::LimitExceeded); }
            batch.validate(&session.scope, validator.as_ref())?;
            if version == 2 { validate_observation_times(&batch, run.checkpoint.observed_at)?; }
            for actor in batch.actors.iter().chain(batch.conversations.iter().flat_map(|c| &c.participants)) {
                if version == 2 && identical_observation(tx, &session.scope, actor.id.as_uuid(), "actor_observations", actor, |_: &mut retract_domain::ActorRecord| {})? { continue; }
                upsert_observation(tx, &session.scope, &actor.resource, "actor_observations", &model::encode(actor)?)?;
            }
            for conversation in &batch.conversations {
                if version == 2 && identical_observation(tx, &session.scope, conversation.id.as_uuid(), "conversation_observations", conversation, |_: &mut retract_domain::ConversationRecord| {})? { continue; }
                if let Some(parent) = conversation.parent_id { check_reference(tx, &session.scope, parent.as_uuid(), ResourceKind::Conversation)?; }
                upsert_observation(tx, &session.scope, &conversation.resource, "conversation_observations", &model::encode(conversation)?)?;
            }
            let mut new_items = 0_u64;
            let mut written = BTreeSet::new();
            for content in &mut batch.contents {
                if !written.insert(content.id) { continue; }
                if version == 2 && identical_observation(tx, &session.scope, content.id.as_uuid(), "content_observations", content, model::clear_derived_fields)? { continue; }
                check_reference(tx, &session.scope, content.conversation_id.as_uuid(), ResourceKind::Conversation)?;
                check_reference(tx, &session.scope, content.author_id.as_uuid(), ResourceKind::Actor)?;
                if let Some(reply) = content.reply_to { check_reference(tx, &session.scope, reply.as_uuid(), ResourceKind::Content)?; }
                if let Some(thread) = content.thread_parent { check_reference(tx, &session.scope, thread.as_uuid(), ResourceKind::Conversation)?; }
                let resource_id = content.id.as_uuid().to_string();
                let exists = tx.query_row("SELECT EXISTS(SELECT 1 FROM content_observations WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4)",
                    params![s[0], s[1], s[2], resource_id], |row| row.get::<_, bool>(0)).map_err(storage)?;
                if !exists { new_items = new_items.checked_add(1).ok_or(ArchiveError::LimitExceeded)?; }
                if progress.committed_items.checked_add(new_items).ok_or(ArchiveError::LimitExceeded)? > self.import_limits.items { return Err(ArchiveError::LimitExceeded); }
                upsert_identity(tx, &content.resource)?;
                model::derive_findings(content);
                let names = content.attachments.iter().filter_map(|a| a.safe_display_name.as_deref()).collect::<Vec<_>>().join("\n");
                tx.execute("INSERT INTO content_observations(provider, account_id, source_id, resource_id, conversation_id, author_id, reply_to_id, thread_parent_id, timestamp_seconds, timestamp_nanos, searchable_text, attachment_names, record_json)
                    VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                    ON CONFLICT(provider, account_id, source_id, resource_id) DO UPDATE SET conversation_id=excluded.conversation_id, author_id=excluded.author_id, reply_to_id=excluded.reply_to_id, thread_parent_id=excluded.thread_parent_id, timestamp_seconds=excluded.timestamp_seconds, timestamp_nanos=excluded.timestamp_nanos, searchable_text=excluded.searchable_text, attachment_names=excluded.attachment_names, record_json=excluded.record_json",
                    params![s[0], s[1], s[2], resource_id, content.conversation_id.as_uuid().to_string(), content.author_id.as_uuid().to_string(), content.reply_to.map(|id| id.as_uuid().to_string()), content.thread_parent.map(|id| id.as_uuid().to_string()), content.timestamp.timestamp(), content.timestamp.timestamp_subsec_nanos(), content.searchable_text, names, model::encode(content)?]).map_err(storage)?;
                for table in ["attachments", "privacy_findings"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4"), params![s[0], s[1], s[2], resource_id]).map_err(storage)?;
                }
                for (ordinal, attachment) in content.attachments.iter().enumerate() {
                    tx.execute("INSERT INTO attachments(provider, account_id, source_id, resource_id, ordinal, record_json) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                        params![s[0], s[1], s[2], resource_id, ordinal as i64, model::encode(attachment)?]).map_err(storage)?;
                }
                for finding in &content.privacy_findings {
                    tx.execute("INSERT INTO privacy_findings(provider, account_id, source_id, resource_id, kind, detector_version) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                        params![s[0], s[1], s[2], resource_id, model::encode(finding)?.trim_matches('"'), content.detector_version]).map_err(storage)?;
                }
            }
            let progress = ImportProgress { phase: ImportPhase::Importing, committed_items: progress.committed_items.checked_add(new_items).ok_or(ArchiveError::LimitExceeded)?, committed_bytes, next_batch: sequence.checked_add(1).ok_or(ArchiveError::LimitExceeded)? };
            tx.execute("INSERT INTO import_batch_receipts(provider, account_id, source_id, run_id, sequence, digest, committed_records, committed_bytes, next_batch, digest_version) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), integer(sequence)?, digest, integer(progress.committed_items)?, integer(progress.committed_bytes)?, integer(progress.next_batch)?, version]).map_err(storage)?;
            if let Some(warnings) = &warnings { append_warnings(tx, &run, sequence, warnings)?; }
            run.checkpoint.progress = progress.clone();
            save_run(tx, &mut run)?;
            Ok(progress)
        }, || session.cancellation.check())
    }

    pub(crate) fn finish_import(
        &mut self,
        session: &ImportSession,
    ) -> Result<ImportProgress, ArchiveError> {
        session.cancellation.check()?;
        let validator = self
            .validators
            .get(&session.scope.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        let progress = self.transaction_checked(
            |tx| {
                let mut run = active_session(
                    tx,
                    session,
                    validator.validation_policy_key().as_str(),
                    self.import_limits,
                )?;
                let missing = missing_mandatory_observations(tx, &session.scope)?;
                run.checkpoint.progress.phase = if missing {
                    ImportPhase::Failed
                } else {
                    ImportPhase::Ready
                };
                run.checkpoint.failure_code =
                    missing.then_some(model::ImportFailureCode::IncompleteSource);
                collect_warnings(tx, &run)?;
                save_run(tx, &mut run)?;
                set_source_phase(tx, &session.scope, run.checkpoint.progress.phase)?;
                Ok(run.checkpoint.progress)
            },
            || session.cancellation.check(),
        )?;
        if progress.phase == ImportPhase::Failed {
            Err(ArchiveError::IncompleteSource)
        } else {
            Ok(progress)
        }
    }
}

pub(super) fn missing_mandatory_observations(
    connection: &Connection,
    scope: &Scope,
) -> Result<bool, ArchiveError> {
    let s = scope_sql(scope);
    connection.query_row("SELECT EXISTS(SELECT 1 FROM content_observations c
        WHERE c.provider=?1 AND c.account_id=?2 AND c.source_id=?3 AND (
            NOT EXISTS(SELECT 1 FROM conversation_observations v WHERE v.provider=c.provider AND v.account_id=c.account_id AND v.source_id=c.source_id AND v.resource_id=c.conversation_id)
            OR NOT EXISTS(SELECT 1 FROM actor_observations a WHERE a.provider=c.provider AND a.account_id=c.account_id AND a.source_id=c.source_id AND a.resource_id=c.author_id)))",
        params![s[0], s[1], s[2]], |row| row.get(0)).map_err(storage)
}

fn upsert_identity(
    tx: &Transaction<'_>,
    resource: &ProviderResourceRef,
) -> Result<(), ArchiveError> {
    let id = resource
        .resource_id()
        .map_err(|_| ArchiveError::InvalidRecord)?
        .to_string();
    let previous = tx
        .query_row(
            "SELECT locator_json FROM resource_identities WHERE resource_id=?",
            [&id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(previous) = previous {
        if model::decode::<ProviderResourceRef>(&previous)? != *resource {
            return Err(ArchiveError::InvalidRecord);
        }
    } else {
        tx.execute("INSERT INTO resource_identities(provider, account_id, resource_id, kind, locator_schema, locator_version, canonical_key, locator_json) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![resource.provider.as_str(), resource.account_id.as_uuid().to_string(), id, model::encode(&resource.resource_kind)?.trim_matches('"'), resource.locator_schema, resource.locator_version, resource.canonical_key, model::encode(resource)?]).map_err(|_| ArchiveError::InvalidRecord)?;
    }
    Ok(())
}

fn upsert_observation(
    tx: &Transaction<'_>,
    scope: &Scope,
    resource: &ProviderResourceRef,
    table: &str,
    json: &str,
) -> Result<(), ArchiveError> {
    upsert_identity(tx, resource)?;
    let s = scope_sql(scope);
    tx.execute(&format!("INSERT INTO {table}(provider, account_id, source_id, resource_id, record_json) VALUES(?1, ?2, ?3, ?4, ?5) ON CONFLICT(provider, account_id, source_id, resource_id) DO UPDATE SET record_json=excluded.record_json"),
        params![s[0], s[1], s[2], resource.resource_id().map_err(|_| ArchiveError::InvalidRecord)?.to_string(), json]).map_err(storage)?;
    Ok(())
}

pub(super) fn check_reference(
    connection: &Connection,
    scope: &Scope,
    id: &Uuid,
    kind: ResourceKind,
) -> Result<(), ArchiveError> {
    let stored = connection
        .query_row(
            "SELECT provider, account_id, kind FROM resource_identities WHERE resource_id=?",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage)?;
    if let Some((provider, account, actual)) = stored {
        if provider != scope.provider.as_str() || account != scope.account_id.as_uuid().to_string()
        {
            return Err(ArchiveError::ScopeMismatch);
        }
        if actual != model::encode(&kind)?.trim_matches('"') {
            return Err(ArchiveError::InvalidRecord);
        }
    }
    Ok(())
}

fn collect_warnings(tx: &Transaction<'_>, run: &Run) -> Result<(), ArchiveError> {
    let s = scope_sql(&run.checkpoint.scope);
    tx.execute("DELETE FROM import_warnings WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4 AND code IN ('missing_reply', 'missing_thread')", params![s[0], s[1], s[2], run.checkpoint.run_id.to_string()]).map_err(storage)?;
    for (code, column, table) in [
        ("missing_reply", "reply_to_id", "content_observations"),
        (
            "missing_thread",
            "thread_parent_id",
            "conversation_observations",
        ),
    ] {
        tx.execute(&format!("INSERT INTO import_warnings(provider, account_id, source_id, run_id, code, count)
            SELECT ?1, ?2, ?3, ?4, ?5, count(*) FROM content_observations c WHERE c.provider=?1 AND c.account_id=?2 AND c.source_id=?3 AND c.{column} IS NOT NULL
            AND NOT EXISTS(SELECT 1 FROM {table} r WHERE r.provider=c.provider AND r.account_id=c.account_id AND r.source_id=c.source_id AND r.resource_id=c.{column}) HAVING count(*) > 0"),
            params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), code]).map_err(storage)?;
    }
    Ok(())
}

fn validate_observation_times(
    batch: &ImportBatch,
    observed: chrono::DateTime<chrono::Utc>,
) -> Result<(), ArchiveError> {
    if batch
        .actors
        .iter()
        .chain(batch.conversations.iter().flat_map(|c| &c.participants))
        .any(|a| a.observed_at != observed)
        || batch
            .conversations
            .iter()
            .any(|c| c.observed_at != observed)
        || batch.contents.iter().any(|c| c.observed_at != observed)
    {
        return Err(ArchiveError::InvalidRecord);
    }
    Ok(())
}

fn identical_observation<T: serde::de::DeserializeOwned + PartialEq>(
    tx: &Transaction<'_>,
    scope: &Scope,
    id: &Uuid,
    table: &str,
    input: &T,
    normalize: impl FnOnce(&mut T),
) -> Result<bool, ArchiveError> {
    let s = scope_sql(scope);
    let stored: Option<String> = tx.query_row(&format!("SELECT record_json FROM {table} WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND resource_id=?4"), params![s[0], s[1], s[2], id.to_string()], |r| r.get(0)).optional().map_err(storage)?;
    let Some(stored) = stored else {
        return Ok(false);
    };
    let mut record: T = model::decode(&stored)?;
    normalize(&mut record);
    if &record != input {
        return Err(ArchiveError::ConflictingObservation);
    }
    Ok(true)
}

fn append_warnings(
    tx: &Transaction<'_>,
    run: &Run,
    sequence: u64,
    warnings: &[model::ImportWarningDelta],
) -> Result<(), ArchiveError> {
    let s = scope_sql(&run.checkpoint.scope);
    for (ordinal, warning) in warnings.iter().enumerate() {
        let count: u64 = tx.query_row("SELECT count FROM import_warnings WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4 AND code=?5", params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), warning.code.as_str()], |r| unsigned(r, 0)).optional().map_err(storage)?.unwrap_or(0);
        let count = integer(
            count
                .checked_add(warning.count)
                .ok_or(ArchiveError::LimitExceeded)?,
        )?;
        tx.execute("INSERT INTO import_warnings VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(provider, account_id, source_id, run_id, code) DO UPDATE SET count=excluded.count", params![s[0], s[1], s[2], run.checkpoint.run_id.to_string(), warning.code.as_str(), count]).map_err(storage)?;
        tx.execute(
            "INSERT INTO import_warning_deltas VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s[0],
                s[1],
                s[2],
                run.checkpoint.run_id.to_string(),
                integer(sequence)?,
                ordinal as i64,
                warning.code.as_str(),
                integer(warning.count)?
            ],
        )
        .map_err(storage)?;
    }
    let count: i64 = tx.query_row("SELECT count(*) FROM import_warnings WHERE provider=?1 AND account_id=?2 AND source_id=?3 AND run_id=?4", params![s[0], s[1], s[2], run.checkpoint.run_id.to_string()], |r| r.get(0)).map_err(storage)?;
    // Two additional closed codes are reserved for finalization references.
    if count > (model::MAX_WARNING_CODES - 2) as i64 {
        return Err(ArchiveError::LimitExceeded);
    }
    Ok(())
}
