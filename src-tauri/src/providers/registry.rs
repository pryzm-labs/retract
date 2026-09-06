use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, RwLock},
};

use retract_domain::ProviderKey;

use super::ports::{ProviderCapability, ProviderRegistration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRegistryError {
    DuplicateProvider(ProviderKey),
    UnknownProvider(ProviderKey),
    InvalidDescriptor(ProviderKey),
    MissingPort {
        provider: ProviderKey,
        capability: ProviderCapability,
    },
    UnsupportedCapability(ProviderCapability),
    StateUnavailable,
}

impl ProviderRegistryError {
    pub fn unsupported(capability: ProviderCapability) -> Self {
        Self::UnsupportedCapability(capability)
    }
}

impl fmt::Display for ProviderRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider(_) => formatter.write_str("provider is already registered"),
            Self::UnknownProvider(_) => formatter.write_str("provider is not registered"),
            Self::InvalidDescriptor(_) => formatter.write_str("provider descriptor is invalid"),
            Self::MissingPort { .. } => {
                formatter.write_str("provider is missing a declared capability port")
            }
            Self::UnsupportedCapability(_) => {
                formatter.write_str("provider capability is unsupported")
            }
            Self::StateUnavailable => formatter.write_str("provider registry is unavailable"),
        }
    }
}

impl std::error::Error for ProviderRegistryError {}

#[derive(Default)]
pub struct ProviderRegistry {
    registrations: RwLock<HashMap<ProviderKey, Arc<dyn ProviderRegistration>>>,
}

impl ProviderRegistry {
    pub fn register(
        &self,
        registration: Arc<dyn ProviderRegistration>,
    ) -> Result<(), ProviderRegistryError> {
        let descriptor = registration.descriptor();
        descriptor.validate()?;
        let mut registrations = self
            .registrations
            .write()
            .map_err(|_| ProviderRegistryError::StateUnavailable)?;
        if registrations.contains_key(&descriptor.key) {
            return Err(ProviderRegistryError::DuplicateProvider(descriptor.key));
        }

        validate_declared_ports(registration.as_ref(), &descriptor)?;
        registrations.insert(descriptor.key.clone(), registration);
        Ok(())
    }

    pub fn get(
        &self,
        key: &ProviderKey,
    ) -> Result<Arc<dyn ProviderRegistration>, ProviderRegistryError> {
        self.registrations
            .read()
            .map_err(|_| ProviderRegistryError::StateUnavailable)?
            .get(key)
            .cloned()
            .ok_or_else(|| ProviderRegistryError::UnknownProvider(key.clone()))
    }
}

fn validate_declared_ports(
    registration: &dyn ProviderRegistration,
    descriptor: &super::ports::ProviderDescriptor,
) -> Result<(), ProviderRegistryError> {
    let checks = [
        (
            descriptor.capabilities.iter().any(|capability| {
                matches!(
                    capability,
                    ProviderCapability::ConversationListing
                        | ProviderCapability::ContentSearch
                        | ProviderCapability::MediaMetadata
                        | ProviderCapability::ExternalLocation
                )
            }),
            registration.query_source().map(|_| ()),
            ProviderCapability::ContentSearch,
        ),
        (
            descriptor.capabilities.iter().any(|capability| {
                matches!(
                    capability,
                    ProviderCapability::AutomaticRemediation
                        | ProviderCapability::BulkRemediation
                        | ProviderCapability::Verification
                )
            }),
            registration.remediation().map(|_| ()),
            ProviderCapability::AutomaticRemediation,
        ),
        (
            descriptor
                .capabilities
                .contains(&ProviderCapability::LiveConnection),
            registration.live_connection().map(|_| ()),
            ProviderCapability::LiveConnection,
        ),
        (
            descriptor.capabilities.iter().any(|capability| {
                matches!(
                    capability,
                    ProviderCapability::ArchiveImport | ProviderCapability::ImportInspection
                )
            }),
            registration.import_inspector().map(|_| ()),
            ProviderCapability::ImportInspection,
        ),
    ];

    for (required, result, capability) in checks {
        if required && result.is_err() {
            return Err(ProviderRegistryError::MissingPort {
                provider: descriptor.key.clone(),
                capability,
            });
        }
    }
    Ok(())
}
