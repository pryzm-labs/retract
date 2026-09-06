use std::collections::{HashMap, HashSet};

use cleaner_domain::PlanOperation;
use retract_domain::{
    ErrorCode, LegacyHistoryRecord, LegacyOperation, LegacyTerminalStatus, SafeError,
};
use sha2::{Digest, Sha256};

use super::model::{
    FoundationState, LegacyHistoryEntry, LegacyStoreFormat, MigrationProvenance, StoreBinding,
};
use crate::{error::AppError, model::PersistedState};

pub(super) fn migrate_legacy(
    legacy: PersistedState,
    binding: StoreBinding,
    format: LegacyStoreFormat,
    source: &[u8],
) -> Result<FoundationState, AppError> {
    let mut plan_operations = HashMap::new();
    for plan in &legacy.plans {
        if plan.id.is_nil() || plan_operations.insert(plan.id, plan.operation).is_some() {
            return Err(AppError::SecureStore(
                "legacy state contains invalid plans".into(),
            ));
        }
    }

    let mut job_ids = HashSet::new();
    let mut history = Vec::with_capacity(legacy.jobs.len());
    for job in legacy.jobs {
        if job.id.is_nil()
            || job.plan_id.is_nil()
            || !job_ids.insert(job.id)
            || job.updated_at < job.created_at
            || plan_operations.get(&job.plan_id) != Some(&job.operation)
        {
            return Err(AppError::SecureStore(
                "legacy state contains invalid jobs".into(),
            ));
        }
        let nonterminal = !job.status.is_terminal();
        let status = match job.status {
            crate::model::JobStatus::Completed => LegacyTerminalStatus::Completed,
            crate::model::JobStatus::Partial => LegacyTerminalStatus::Partial,
            crate::model::JobStatus::Failed => LegacyTerminalStatus::Failed,
            crate::model::JobStatus::Cancelled => LegacyTerminalStatus::Cancelled,
            crate::model::JobStatus::Queued | crate::model::JobStatus::Running
                if job.deleted > 0 =>
            {
                LegacyTerminalStatus::Partial
            }
            crate::model::JobStatus::Queued | crate::model::JobStatus::Running => {
                LegacyTerminalStatus::Failed
            }
        };
        let mut diagnostics = job
            .error_codes
            .iter()
            .filter_map(|code| safe_legacy_code(code))
            .map(|code| SafeError {
                code,
                retry_at: None,
            })
            .collect::<Vec<_>>();
        if nonterminal
            && !diagnostics
                .iter()
                .any(|error| error.code == ErrorCode::MigrationRequiresNewReview)
        {
            diagnostics.push(SafeError {
                code: ErrorCode::MigrationRequiresNewReview,
                retry_at: None,
            });
        }
        history.push(LegacyHistoryEntry {
            record: LegacyHistoryRecord {
                id: job.id,
                plan_id: job.plan_id,
                operation: legacy_operation(job.operation),
                status,
                total: count(job.total)?,
                deleted: count(job.deleted)?,
                skipped: count(job.skipped)?,
                failed: count(job.failed)?,
                next_batch: count(job.next_batch)?,
                diagnostics,
                created_at: job.created_at,
                updated_at: job.updated_at,
            },
            executable: false,
        });
    }

    let digest = Sha256::digest(source);
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let state = FoundationState {
        schema_version: super::model::FOUNDATION_SCHEMA_VERSION,
        binding,
        identities: Vec::new(),
        sources: Vec::new(),
        plans: Vec::new(),
        jobs: Vec::new(),
        legacy_history: history,
        migration: Some(MigrationProvenance {
            source_format: format,
            source_sha256: format!("sha256:{digest}"),
            source_bytes: u64::try_from(source.len()).map_err(|_| {
                AppError::SecureStore("legacy migration source is too large".into())
            })?,
        }),
    };
    state.validate(&state.binding)?;
    Ok(state)
}

fn count(value: usize) -> Result<u64, AppError> {
    u64::try_from(value).map_err(|_| AppError::SecureStore("legacy counter is too large".into()))
}

fn legacy_operation(operation: PlanOperation) -> LegacyOperation {
    match operation {
        PlanOperation::SelectedMessages => LegacyOperation::SelectedMessages,
        PlanOperation::DeleteMyMessages => LegacyOperation::DeleteMyMessages,
        PlanOperation::ClearHistory => LegacyOperation::ClearHistory,
        PlanOperation::ClearHistoryAndLeave => LegacyOperation::ClearHistoryAndLeave,
        PlanOperation::DeleteAllMessagesAndLeave => LegacyOperation::DeleteAllMessagesAndLeave,
        PlanOperation::RemoveChatForSelf => LegacyOperation::RemoveChatForSelf,
        PlanOperation::DeleteBySender => LegacyOperation::DeleteBySender,
        PlanOperation::DeleteGroup => LegacyOperation::DeleteGroup,
        PlanOperation::LeaveChat => LegacyOperation::LeaveChat,
    }
}

fn safe_legacy_code(value: &str) -> Option<ErrorCode> {
    let value = value.strip_prefix("telegram_").unwrap_or(value);
    Some(match value {
        "authentication_required" => ErrorCode::AuthenticationRequired,
        "permission_changed" => ErrorCode::PermissionChanged,
        "not_found" => ErrorCode::NotFound,
        "already_removed" => ErrorCode::AlreadyRemoved,
        "rate_limited" => ErrorCode::RateLimited,
        "cost_limit_reached" => ErrorCode::CostLimitReached,
        "transient" => ErrorCode::Transient,
        "permanent" => ErrorCode::Permanent,
        "ambiguous_outcome" => ErrorCode::AmbiguousOutcome,
        "unsupported_schema" => ErrorCode::UnsupportedSchema,
        "invalid_archive" => ErrorCode::InvalidArchive,
        "scope_mismatch" => ErrorCode::ScopeMismatch,
        "stale_context" => ErrorCode::StaleContext,
        "identity_unavailable" => ErrorCode::IdentityUnavailable,
        "migration_requires_new_review" => ErrorCode::MigrationRequiresNewReview,
        "restart_requires_new_review" => ErrorCode::RestartRequiresNewReview,
        _ => return None,
    })
}
