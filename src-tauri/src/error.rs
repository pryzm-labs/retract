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
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("system authentication failed: {0}")]
    SystemAuthentication(String),
    #[error("internal state is unavailable")]
    StateUnavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<AppError> for CommandError {
    fn from(value: AppError) -> Self {
        let code = match value {
            AppError::Domain(_) | AppError::InvalidRequest(_) => "invalid_request",
            AppError::Gateway(_) => "telegram_error",
            AppError::Timeout(_) => "timeout",
            AppError::NotFound => "not_found",
            AppError::JobAlreadyTerminal => "job_terminal",
            AppError::SecureStore(_) => "secure_store_error",
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
