use retract_domain::{ErrorCode, SafeError};

use crate::error::AppError;

pub(crate) fn safe_diagnostic(code: &str) -> SafeError {
    SafeError {
        code: match code {
            "state_persistence_failed" | "secure_store" => ErrorCode::StatePersistenceFailed,
            "stale_context" => ErrorCode::StaleContext,
            "ambiguous_outcome" => ErrorCode::AmbiguousOutcome,
            "scope_mismatch" => ErrorCode::ScopeMismatch,
            "restart_requires_new_review" => ErrorCode::RestartRequiresNewReview,
            "not_found" => ErrorCode::NotFound,
            "telegram_timeout" => ErrorCode::Transient,
            "telegram_rate_limited" => ErrorCode::RateLimited,
            "authentication_required" | "system_authentication" => {
                ErrorCode::AuthenticationRequired
            }
            "already_removed" => ErrorCode::AlreadyRemoved,
            "cost_limit_reached" => ErrorCode::CostLimitReached,
            "permanent" | "invalid_plan" => ErrorCode::Permanent,
            "unsupported_schema" => ErrorCode::UnsupportedSchema,
            "invalid_archive" => ErrorCode::InvalidArchive,
            "unsupported_contract_version" => ErrorCode::UnsupportedContractVersion,
            "identity_unavailable" => ErrorCode::IdentityUnavailable,
            "profile_in_use" => ErrorCode::ProfileInUse,
            "migration_requires_new_review" | "legacy_store_requires_new_review" => {
                ErrorCode::MigrationRequiresNewReview
            }
            _ => ErrorCode::PermissionChanged,
        },
        retry_at: None,
    }
}

pub(crate) fn legacy_diagnostic_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::AuthenticationRequired => "authentication_required",
        ErrorCode::PermissionChanged => "telegram_rejected",
        ErrorCode::NotFound => "not_found",
        ErrorCode::AlreadyRemoved => "already_removed",
        ErrorCode::RateLimited => "telegram_rate_limited",
        ErrorCode::CostLimitReached => "cost_limit_reached",
        ErrorCode::Transient => "telegram_timeout",
        ErrorCode::Permanent => "permanent",
        ErrorCode::AmbiguousOutcome => "ambiguous_outcome",
        ErrorCode::UnsupportedSchema => "unsupported_schema",
        ErrorCode::InvalidArchive => "invalid_archive",
        ErrorCode::UnsupportedContractVersion => "unsupported_contract_version",
        ErrorCode::ScopeMismatch => "scope_mismatch",
        ErrorCode::StaleContext => "stale_context",
        ErrorCode::IdentityUnavailable => "identity_unavailable",
        ErrorCode::ProfileInUse => "profile_in_use",
        ErrorCode::StatePersistenceFailed => "state_persistence_failed",
        ErrorCode::MigrationRequiresNewReview => "migration_requires_new_review",
        ErrorCode::RestartRequiresNewReview => "restart_requires_new_review",
    }
}

pub(crate) fn invalid_recipe() -> AppError {
    AppError::InvalidRequest("invalid Telegram execution recipe".into())
}
