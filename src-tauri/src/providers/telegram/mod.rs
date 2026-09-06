pub mod application;
pub mod compat;
pub mod connection;
pub mod diagnostics;
pub mod engine_context;
pub mod identity;
pub mod locators;
pub mod model;
pub mod native;
pub mod normalize;
pub mod query;
pub mod recipe;
pub mod registration;

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod query_tests;

// Keep the existing Telegram gateway reachable as the compatibility boundary
// while the neutral service takes over composition in the later task.
pub use crate::gateway::TelegramGateway;
pub use native::{LiveGateway, LiveGatewayConfig};
