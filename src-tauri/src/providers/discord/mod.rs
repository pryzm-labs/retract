//! Discord archive import, query, account-bound sign-in, and reviewed remediation.
#![allow(dead_code)]
pub(crate) mod application;
pub(crate) mod browser;
pub(crate) mod commands;
pub(crate) mod http;
pub(crate) mod import;
pub(crate) mod locators;
pub(crate) mod model;
mod normalize;
pub(crate) mod progress;
pub(crate) mod remediation;
pub(crate) mod session;

#[cfg(feature = "discord-import-bench")]
pub(crate) mod benchmark;

pub(crate) use locators::DiscordPayloadValidator;
pub(crate) use normalize::DiscordNormalizer;

#[cfg(test)]
mod http_tests;
#[cfg(test)]
pub(crate) mod import_tests;
#[cfg(test)]
mod remediation_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;
