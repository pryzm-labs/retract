pub mod compat;
pub mod engine_context;
pub mod identity;
pub mod locators;

#[cfg(test)]
mod engine_tests;

// Keep the existing Telegram gateway reachable as the compatibility boundary
// while the neutral service takes over composition in the later task.
pub use crate::gateway::TelegramGateway;
pub use crate::live_gateway::{LiveGateway, LiveGatewayConfig};
