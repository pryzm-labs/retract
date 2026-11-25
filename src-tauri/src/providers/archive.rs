//! Production composition of offline archive validators and query connections.
use super::ApplicationConnection;
use crate::{
    RuntimeState,
    persistence::archive::{ArchiveOwner, ArchiveService, ArchiveSourceEntry, ArchiveValidators},
    provider_service::safe,
};
use retract_domain::{ErrorCode, SafeError};
use std::{path::PathBuf, sync::Arc};

pub(crate) fn application_validators() -> ArchiveValidators {
    use crate::providers::discord::{DiscordPayloadValidator, locators::discord_provider_key};
    std::collections::BTreeMap::from([
        (
            discord_provider_key(),
            Arc::new(DiscordPayloadValidator)
                as Arc<dyn crate::persistence::ProviderPayloadValidator>,
        ),
        (
            crate::providers::x::key(),
            Arc::new(crate::providers::x::Validator),
        ),
    ])
}
pub(crate) fn owner(root: PathBuf) -> ArchiveOwner {
    ArchiveOwner::application(root, application_validators())
}
pub(super) fn connect(
    runtime: &RuntimeState,
    entry: ArchiveSourceEntry,
    archives: Arc<ArchiveService>,
) -> Result<Arc<dyn ApplicationConnection>, SafeError> {
    match entry.source.provider.as_str() {
        "discord" => super::discord::application::build_connection(
            &runtime.application_root,
            entry,
            archives,
            runtime.discord_session.clone(),
        ),
        "x" => super::x::build_connection(entry, archives),
        _ => Err(safe(ErrorCode::UnsupportedSchema)),
    }
}
