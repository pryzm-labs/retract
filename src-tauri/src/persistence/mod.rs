mod foundation_store;
mod migration;
mod model;

pub use foundation_store::FoundationStore;
pub use model::{
    FoundationState, LegacyHistoryEntry, LegacyStoreFormat, MigrationProvenance,
    ProviderPayloadValidator, StoreBinding, VerifiedNativeAccountIdentity,
};

#[cfg(test)]
use foundation_store::{RealStoreIo, StoreIo};

#[cfg(test)]
mod tests;
