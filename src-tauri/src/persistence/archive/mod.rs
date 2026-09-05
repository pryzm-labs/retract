mod codec;
mod error;

#[allow(unused_imports)]
pub(crate) use codec::ArchiveKey;
pub(crate) use error::ArchiveError;

#[cfg(test)]
mod codec_tests;
