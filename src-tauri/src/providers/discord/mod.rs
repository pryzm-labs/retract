//! Backend-only archive codecs. No usable provider, connection or action ports.
// The coordinator consumes these entry points in the next implementation stage.
#![allow(dead_code)]
pub(crate) mod locators;
pub(crate) mod model;
mod normalize;

pub(crate) use locators::DiscordPayloadValidator;
#[allow(unused_imports)] // Backend coordinator is added in the next stage.
pub(crate) use normalize::DiscordNormalizer;

#[cfg(test)]
mod tests;
