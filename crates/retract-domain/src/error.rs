use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("Invalid provider key")]
    InvalidProviderKey,
    #[error("Invalid identity")]
    InvalidIdentity,
    #[error("Invalid resource reference")]
    InvalidReference,
    #[error("Resource scope does not match")]
    ScopeMismatch,
    #[error("Conflicting resource references")]
    ConflictingReference,
    #[error("Invalid remediation plan")]
    InvalidPlan,
    #[error("Plan fingerprint does not match")]
    FingerprintMismatch,
    #[error("Invalid confirmation requirements")]
    InvalidConfirmation,
    #[error("Invalid job record")]
    InvalidJob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AuthenticationRequired,
    PermissionChanged,
    NotFound,
    AlreadyRemoved,
    RateLimited,
    CostLimitReached,
    Transient,
    Permanent,
    AmbiguousOutcome,
    UnsupportedSchema,
    InvalidArchive,
    UnsupportedContractVersion,
    ScopeMismatch,
    StaleContext,
    IdentityUnavailable,
    ProfileInUse,
    StatePersistenceFailed,
    MigrationRequiresNewReview,
    RestartRequiresNewReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    AuthenticationRequired,
    PermissionChanged,
    NotFound,
    AlreadyRemoved,
    RateLimited,
    CostLimitReached,
    Transient,
    Permanent,
    AmbiguousOutcome,
    UnsupportedSchema,
    InvalidArchive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationErrorKind {
    UnsupportedContractVersion,
    ScopeMismatch,
    StaleContext,
    IdentityUnavailable,
    ProfileInUse,
    StatePersistenceFailed,
    MigrationRequiresNewReview,
    RestartRequiresNewReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProviderErrorWire", into = "ProviderErrorWire")]
pub struct ProviderError {
    pub code: ProviderErrorKind,
    pub retry_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderErrorWire {
    code: ProviderErrorKind,
    retry_at: Option<DateTime<Utc>>,
    message: Option<String>,
}

impl ProviderError {
    pub fn message(&self) -> &'static str {
        match self.code {
            ProviderErrorKind::AuthenticationRequired => "Sign in to continue.",
            ProviderErrorKind::PermissionChanged => {
                "Permission to perform this action has changed."
            }
            ProviderErrorKind::NotFound => "The requested resource could not be found.",
            ProviderErrorKind::AlreadyRemoved => "The resource has already been removed.",
            ProviderErrorKind::RateLimited => "The provider requires a wait before continuing.",
            ProviderErrorKind::CostLimitReached => "The configured cost limit has been reached.",
            ProviderErrorKind::Transient => "The provider is temporarily unavailable.",
            ProviderErrorKind::Permanent => "The provider could not complete this action.",
            ProviderErrorKind::AmbiguousOutcome => {
                "The action outcome is uncertain. Review before retrying."
            }
            ProviderErrorKind::UnsupportedSchema => "This data version is not supported.",
            ProviderErrorKind::InvalidArchive => "The archive could not be validated.",
        }
    }
}
impl From<ProviderError> for ProviderErrorWire {
    fn from(value: ProviderError) -> Self {
        Self {
            code: value.code,
            retry_at: value.retry_at,
            message: Some(value.message().into()),
        }
    }
}
impl TryFrom<ProviderErrorWire> for ProviderError {
    type Error = &'static str;
    fn try_from(value: ProviderErrorWire) -> Result<Self, Self::Error> {
        let error = Self {
            code: value.code,
            retry_at: value.retry_at,
        };
        if (error.code == ProviderErrorKind::RateLimited) != error.retry_at.is_some() {
            return Err("Retry timing is valid only and always for rate limits");
        }
        if value
            .message
            .as_ref()
            .is_some_and(|message| message != error.message())
        {
            return Err("Provider error message does not match its safe code");
        }
        Ok(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ApplicationErrorWire", into = "ApplicationErrorWire")]
pub struct ApplicationError {
    pub code: ApplicationErrorKind,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplicationErrorWire {
    code: ApplicationErrorKind,
    message: Option<String>,
}
impl ApplicationError {
    pub fn message(&self) -> &'static str {
        match self.code {
            ApplicationErrorKind::UnsupportedContractVersion => {
                "Reload the application to use the current interface."
            }
            ApplicationErrorKind::ScopeMismatch => {
                "This action belongs to a different account or source."
            }
            ApplicationErrorKind::StaleContext => {
                "The connection has changed. Review this action again."
            }
            ApplicationErrorKind::IdentityUnavailable => {
                "The account identity could not be verified."
            }
            ApplicationErrorKind::ProfileInUse => {
                "This profile is already in use by another application process."
            }
            ApplicationErrorKind::StatePersistenceFailed => {
                "Progress could not be saved. No further actions were scheduled."
            }
            ApplicationErrorKind::MigrationRequiresNewReview => {
                "Legacy history is preserved. A new review is required."
            }
            ApplicationErrorKind::RestartRequiresNewReview => {
                "This interrupted action requires a new review."
            }
        }
    }
}
impl From<ApplicationError> for ApplicationErrorWire {
    fn from(value: ApplicationError) -> Self {
        Self {
            code: value.code,
            message: Some(value.message().into()),
        }
    }
}
impl TryFrom<ApplicationErrorWire> for ApplicationError {
    type Error = &'static str;
    fn try_from(value: ApplicationErrorWire) -> Result<Self, Self::Error> {
        let error = Self { code: value.code };
        if value
            .message
            .as_ref()
            .is_some_and(|message| message != error.message())
        {
            return Err("Application error message does not match its safe code");
        }
        Ok(error)
    }
}

/// Only predefined diagnostics cross the application boundary. Message text is
/// derived from the code, never accepted from an adapter or persisted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SafeErrorWire", into = "SafeErrorWire")]
pub struct SafeError {
    pub code: ErrorCode,
    pub retry_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SafeErrorWire {
    code: ErrorCode,
    retry_at: Option<DateTime<Utc>>,
    message: Option<String>,
}

impl From<SafeError> for SafeErrorWire {
    fn from(value: SafeError) -> Self {
        Self {
            code: value.code,
            retry_at: value.retry_at,
            message: Some(value.message().into()),
        }
    }
}

impl TryFrom<SafeErrorWire> for SafeError {
    type Error = &'static str;
    fn try_from(value: SafeErrorWire) -> Result<Self, Self::Error> {
        let error = Self {
            code: value.code,
            retry_at: value.retry_at,
        };
        if value
            .message
            .as_ref()
            .is_some_and(|message| message != error.message())
        {
            return Err("Diagnostic message does not match its safe code");
        }
        Ok(error)
    }
}

impl SafeError {
    pub fn message(&self) -> &'static str {
        match self.code {
            ErrorCode::AuthenticationRequired => "Sign in to continue.",
            ErrorCode::PermissionChanged => "Permission to perform this action has changed.",
            ErrorCode::NotFound => "The requested resource could not be found.",
            ErrorCode::AlreadyRemoved => "The resource has already been removed.",
            ErrorCode::RateLimited => "The provider requires a wait before continuing.",
            ErrorCode::CostLimitReached => "The configured cost limit has been reached.",
            ErrorCode::Transient => "The provider is temporarily unavailable.",
            ErrorCode::Permanent => "The provider could not complete this action.",
            ErrorCode::AmbiguousOutcome => {
                "The action outcome is uncertain. Review before retrying."
            }
            ErrorCode::UnsupportedSchema => "This data version is not supported.",
            ErrorCode::InvalidArchive => "The archive could not be validated.",
            ErrorCode::UnsupportedContractVersion => {
                "Reload the application to use the current interface."
            }
            ErrorCode::ScopeMismatch => "This action belongs to a different account or source.",
            ErrorCode::StaleContext => "The connection has changed. Review this action again.",
            ErrorCode::IdentityUnavailable => "The account identity could not be verified.",
            ErrorCode::ProfileInUse => {
                "This profile is already in use by another application process."
            }
            ErrorCode::StatePersistenceFailed => {
                "Progress could not be saved. No further actions were scheduled."
            }
            ErrorCode::MigrationRequiresNewReview => {
                "Legacy history is preserved. A new review is required."
            }
            ErrorCode::RestartRequiresNewReview => "This interrupted action requires a new review.",
        }
    }
}
