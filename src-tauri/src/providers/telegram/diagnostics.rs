use retract_domain::{ErrorCode, SafeError};

use crate::error::AppError;

/// Telegram-native failures are interpreted at the provider boundary. Shared
/// storage and authentication variants retain the core mapping.
pub(crate) fn boundary_error(error: AppError) -> SafeError {
    match error {
        AppError::Gateway(ref message) if message == "RETRACT_AMBIGUOUS_OUTCOME" => {
            crate::provider_service::safe(ErrorCode::AmbiguousOutcome)
        }
        AppError::Gateway(_) => crate::provider_service::safe(ErrorCode::PermissionChanged),
        AppError::Timeout(_) => crate::provider_service::safe(ErrorCode::Transient),
        shared => crate::error::boundary_error(shared),
    }
}

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

pub(crate) fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Gateway(message) if message == "RETRACT_AMBIGUOUS_OUTCOME" => "ambiguous_outcome",
        AppError::Gateway(message)
            if matches!(
                message.as_str(),
                "TDLIB_REQUEST_TIMEOUT" | "TDLIB_RESPONSE_CHANNEL_CLOSED"
            ) =>
        {
            "telegram_timeout"
        }
        AppError::InvalidRequest(message) if message == "stale_context" => "stale_context",
        AppError::Gateway(_) => "telegram_rejected",
        AppError::Timeout(_) => "telegram_timeout",
        AppError::SecureStore(_) => "secure_store",
        AppError::SystemAuthentication(_) => "system_authentication",
        AppError::NotFound => "not_found",
        AppError::JobAlreadyTerminal => "job_terminal",
        AppError::Domain(_) | AppError::InvalidRequest(_) => "invalid_plan",
        AppError::StateUnavailable => "state_unavailable",
        AppError::ProfileInUse => "profile_in_use",
        AppError::StatePersistenceFailed => "state_persistence_failed",
    }
}

pub(crate) fn telegram_retry_after(error: &AppError) -> Option<u64> {
    let AppError::Gateway(message) = error else {
        return None;
    };
    let normalized = message.to_ascii_lowercase();
    if !normalized.contains("flood_wait")
        && !normalized.contains("retry after")
        && !normalized.contains("too many requests")
        && !normalized.contains("429")
    {
        return None;
    }
    let seconds = message
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u64>().ok())
        .rfind(|number| *number != 429)
        .unwrap_or(5);
    Some(seconds.clamp(1, 86_400))
}
