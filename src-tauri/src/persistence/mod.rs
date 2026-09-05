#[allow(dead_code)]
pub(crate) mod archive;
mod foundation_store;
mod migration;
mod model;

pub use foundation_store::FoundationStore;
pub use model::{
    FoundationState, LegacyHistoryEntry, LegacyStoreFormat, MigrationProvenance,
    ProviderPayloadValidator, ProviderValidationPolicyKey, StoreBinding,
    VerifiedNativeAccountIdentity,
};

#[cfg(test)]
use foundation_store::{RealStoreIo, StoreIo};

#[cfg(test)]
mod tests;
