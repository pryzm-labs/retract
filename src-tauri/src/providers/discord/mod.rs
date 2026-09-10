//! Backend-only archive normalization and owned import. No usable provider ports.
// Kept backend-only until a separately reviewed UI/IPC integration.
#![allow(dead_code)]
pub(crate) mod import;
pub(crate) mod locators;
pub(crate) mod model;
mod normalize;
pub(crate) mod progress;

pub(crate) use locators::DiscordPayloadValidator;
pub(crate) use normalize::DiscordNormalizer;

#[cfg(test)]
pub(crate) mod import_tests;
#[cfg(test)]
mod tests;
