mod codec;
mod error;
mod model;
mod schema;
mod store;

#[allow(unused_imports)]
pub(crate) use codec::ArchiveKey;
pub(crate) use error::ArchiveError;
#[allow(unused_imports)]
pub(crate) use store::ArchiveStore;

#[cfg(test)]
mod codec_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(super) use store::test_support;
