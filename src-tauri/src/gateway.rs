#[allow(unused_imports)]
pub use crate::providers::telegram::native::ports::{
    GatewayInfo, TelegramConnectionIo, TelegramMutation, TelegramRead, TelegramSession,
};

/// Transitional compatibility surface for the legacy executor and facade.
pub trait TelegramGateway: TelegramRead + TelegramMutation + TelegramConnectionIo {}

impl<T> TelegramGateway for T where
    T: TelegramRead + TelegramMutation + TelegramConnectionIo + ?Sized
{
}
