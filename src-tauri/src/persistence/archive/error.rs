use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveError {
    UnsupportedCodec,
    UnavailableKey,
    InvalidStore,
    UnsupportedSchema,
    StoreInUse,
    ScopeMismatch,
    InvalidRecord,
    LimitExceeded,
    Busy,
    IncompleteSource,
    StaleCursor,
    Cancelled,
    StorageFailure,
    CleanupPending,
}

impl ArchiveError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedCodec => "unsupported_codec",
            Self::UnavailableKey => "unavailable_key",
            Self::InvalidStore => "invalid_store",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::StoreInUse => "store_in_use",
            Self::ScopeMismatch => "scope_mismatch",
            Self::InvalidRecord => "invalid_record",
            Self::LimitExceeded => "limit_exceeded",
            Self::Busy => "busy",
            Self::IncompleteSource => "incomplete_source",
            Self::StaleCursor => "stale_cursor",
            Self::Cancelled => "cancelled",
            Self::StorageFailure => "storage_failure",
            Self::CleanupPending => "cleanup_pending",
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ArchiveError {}
