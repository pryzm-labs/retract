mod codec;
mod error;
mod ingest;
mod ingest_state;
mod model;
mod preflight;
mod schema;
mod store;

#[allow(unused_imports)]
pub(crate) use codec::ArchiveKey;
pub(crate) use error::ArchiveError;
#[allow(unused_imports)]
pub(crate) use model::{
    ImportBatch, ImportCancellation, ImportCheckpoint, ImportPhase, ImportProgress, ImportSession,
};
#[allow(unused_imports)]
pub(crate) use store::ArchiveStore;

#[cfg(test)]
mod codec_tests;
#[cfg(test)]
mod ingest_state_tests;
#[cfg(test)]
mod ingest_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(super) use store::test_support;
