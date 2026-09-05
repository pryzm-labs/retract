//! Archive-only structural validation and persisted record encoding.

use std::io::{self, Write};

use retract_domain::{AccountRecord, ConnectionState, SourceKind, SourceRecord};
use serde::{Serialize, de::DeserializeOwned};

use crate::persistence::{ProviderPayloadValidator, VerifiedNativeAccountIdentity};

use super::ArchiveError;

const ENVELOPE_BYTES: usize = 64 * 1024;

/// Counts the actual JSON encoding without retaining a serialized buffer and
/// aborts serialization as soon as the approved byte ceiling is exceeded.
pub(super) fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize, ArchiveError> {
    struct Counter {
        bytes: usize,
        limit: usize,
        exceeded: bool,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit - self.bytes {
                self.exceeded = true;
                return Err(io::Error::other("archive encoded size limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        limit,
        exceeded: false,
    };
    serde_json::to_writer(&mut counter, value).map_err(|_| {
        if counter.exceeded {
            ArchiveError::LimitExceeded
        } else {
            ArchiveError::InvalidRecord
        }
    })?;
    Ok(counter.bytes)
}

pub(super) fn validate_account_envelopes(account: &AccountRecord) -> Result<(), ArchiveError> {
    encoded_size(&account.native_identity, ENVELOPE_BYTES)?;
    if let Some(avatar) = &account.avatar {
        encoded_size(avatar, ENVELOPE_BYTES)?;
    }
    Ok(())
}

pub(super) fn validate_archive_account(
    account: &AccountRecord,
    validator: &dyn ProviderPayloadValidator,
) -> Result<VerifiedNativeAccountIdentity, ArchiveError> {
    if account.connection_state != ConnectionState::Disconnected {
        return Err(ArchiveError::InvalidRecord);
    }
    validate_account_envelopes(account)?;
    account
        .validate()
        .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_account(account)
        .map_err(|_| ArchiveError::InvalidRecord)
}

pub(super) fn validate_registration(
    account: &AccountRecord,
    source: &SourceRecord,
    validator: &dyn ProviderPayloadValidator,
) -> Result<VerifiedNativeAccountIdentity, ArchiveError> {
    if source.provider != account.provider || source.account_id != account.id {
        return Err(ArchiveError::ScopeMismatch);
    }
    if source.kind != SourceKind::ArchiveImport {
        return Err(ArchiveError::InvalidRecord);
    }
    let native = validate_archive_account(account, validator)?;
    encoded_size(&source.schema_profile, ENVELOPE_BYTES)?;
    source
        .validate(account)
        .map_err(|_| ArchiveError::InvalidRecord)?;
    validator
        .validate_source(source, account)
        .map_err(|_| ArchiveError::InvalidRecord)?;
    Ok(native)
}

pub(super) fn encode(record: &impl Serialize) -> Result<String, ArchiveError> {
    serde_json::to_string(record).map_err(|_| ArchiveError::InvalidRecord)
}

pub(super) fn decode<T: DeserializeOwned>(record: &str) -> Result<T, ArchiveError> {
    serde_json::from_str(record).map_err(|_| ArchiveError::InvalidRecord)
}
