#[cfg(test)]
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Domain(#[from] cleaner_domain::DomainError),
    #[error("Telegram rejected the request: {0}")]
    Gateway(String),
    #[error("{0}")]
    Timeout(String),
    #[error("the requested record was not found")]
    NotFound,
    #[error("the deletion job is already terminal")]
    JobAlreadyTerminal,
    #[error("secure local storage failed: {0}")]
    SecureStore(String),
    #[error("This profile is already in use by another application process.")]
    ProfileInUse,
    #[error("Progress could not be saved. No further actions were scheduled.")]
    StatePersistenceFailed,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("system authentication failed: {0}")]
    SystemAuthentication(String),
    #[error("internal state is unavailable")]
    StateUnavailable,
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

#[cfg(test)]
impl From<AppError> for CommandError {
    fn from(value: AppError) -> Self {
        let code = match value {
            AppError::Domain(_) | AppError::InvalidRequest(_) => "invalid_request",
            AppError::Gateway(_) => "telegram_error",
            AppError::Timeout(_) => "timeout",
            AppError::NotFound => "not_found",
            AppError::JobAlreadyTerminal => "job_terminal",
            AppError::SecureStore(_) => "secure_store_error",
            AppError::ProfileInUse => "profile_in_use",
            AppError::StatePersistenceFailed => "state_persistence_failed",
            AppError::SystemAuthentication(_) => "system_authentication_failed",
            AppError::StateUnavailable => "state_unavailable",
        };
        Self {
            code,
            message: value.to_string(),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::SecureStore(value.to_string())
    }
}

/// Shared infrastructure errors are never serialized at the v2 boundary.
pub(crate) fn boundary_error(error: AppError) -> retract_domain::SafeError {
    use retract_domain::ErrorCode;
    let code = match error {
        AppError::ProfileInUse => ErrorCode::ProfileInUse,
        AppError::StatePersistenceFailed
        | AppError::SecureStore(_)
        | AppError::StateUnavailable => ErrorCode::StatePersistenceFailed,
        AppError::InvalidRequest(ref message) if message == "stale_context" => {
            ErrorCode::StaleContext
        }
        AppError::NotFound => ErrorCode::NotFound,
        AppError::SystemAuthentication(_) => ErrorCode::AuthenticationRequired,
        AppError::Domain(_) | AppError::InvalidRequest(_) => ErrorCode::ScopeMismatch,
        AppError::Gateway(_) | AppError::Timeout(_) | AppError::JobAlreadyTerminal => {
            ErrorCode::PermissionChanged
        }
    };
    crate::provider_service::safe(code)
}
