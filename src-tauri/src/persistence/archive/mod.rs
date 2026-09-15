#[cfg(feature = "archive-bench")]
pub(crate) mod benchmark;
mod codec;
mod error;
mod ingest;
mod ingest_state;
mod lifecycle;
mod migration;
mod migration_validation;
mod model;
mod preflight;
mod query;
mod remove;
mod schema;
mod store;
mod worker;

#[allow(unused_imports)]
pub(crate) use codec::ArchiveKey;
pub(crate) use error::ArchiveError;
pub(crate) use lifecycle::ArchiveOwner;
#[allow(unused_imports)]
pub(crate) use model::{
    ArchiveImportResolution, ArchiveSearch, ArchiveSourceEntry, ENVELOPE_BYTES, ImportBatch,
    ImportBatchV2, ImportCancellation, ImportCheckpoint, ImportDisposition, ImportFailureCode,
    ImportPhase, ImportProgress, ImportSession, ImportWarningCode, ImportWarningDelta,
    MAX_ATTACHMENTS, MAX_BATCH_BYTES, MAX_BATCH_RECORDS, MAX_SEARCHABLE_BYTES, NewArchiveImport,
    RemovalOutcome, encoded_size,
};
#[allow(unused_imports)]
pub(crate) use store::ArchiveStore;
#[allow(unused_imports)]
pub(crate) use worker::{ArchiveQuerySource, ArchiveService};
#[cfg(test)]
mod import_v2_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod worker_tests;

#[cfg(test)]
mod codec_tests;
#[cfg(test)]
mod ingest_state_tests;
#[cfg(test)]
mod ingest_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod remove_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(super) use store::test_support;
