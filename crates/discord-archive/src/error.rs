use thiserror::Error;

/// Fixed diagnostics: never retain upstream messages, paths, or input snippets.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ArchiveError {
    #[error("unsupported Discord profile")]
    UnsupportedProfile,
    #[error("invalid Discord profile")]
    InvalidProfile,
    #[error("invalid archive")]
    InvalidArchive,
    #[error("unsafe entry name")]
    UnsafeEntryName,
    #[error("unsupported archive feature")]
    UnsupportedFeature,
    #[error("ZIP64 is not supported by this policy")]
    UnsupportedZip64,
    #[error("archive integrity failure")]
    IntegrityFailure,
    #[error("resource limit exceeded")]
    LimitExceeded,
    #[error("invalid resource limits")]
    InvalidLimits,
    #[error("operation cancelled")]
    Cancelled,
    #[error("invalid entry selection")]
    InvalidSelection,
    #[error("input read failed")]
    ReadFailure,
    #[error("invalid JSON structure")]
    InvalidJson,
    #[error("duplicate JSON key")]
    DuplicateJsonKey,
}
