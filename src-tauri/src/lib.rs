pub mod compatibility;
mod connection_settings;
#[cfg(test)]
mod demo_gateway;
mod error;
#[cfg(test)]
mod foundation_lifecycle_tests;
mod gateway;
mod live_gateway;
mod local_auth;
mod model;
pub mod persistence;
pub mod provider_service;
pub mod providers;
mod secure_store;
mod service;
#[cfg(test)]
mod setup_gateway;
mod tdjson;
#[cfg(feature = "archive-bench")]
pub use persistence::archive::benchmark::run_archive_storage_benchmark;

use provider_service::ProviderService;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager;
use tokio::sync::RwLock;

/// Read leases span each command; settings obtains the exclusive lease before
/// inspecting workers, invalidating the old service and installing a replacement.
pub(crate) struct RuntimeState {
    pub(crate) service: RwLock<Arc<ProviderService>>,
    pub(crate) archives: persistence::archive::ArchiveOwner,
}
impl RuntimeState {
    #[cfg(test)]
    pub(crate) fn new(service: Arc<ProviderService>) -> Self {
        Self {
            service: RwLock::new(service),
            archives: persistence::archive::ArchiveOwner::application(std::path::PathBuf::new()),
        }
    }
}

fn create_service<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<ProviderService>, error::AppError> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|_| error::AppError::StatePersistenceFailed)?;
    secure_store::bind_application_root(base.clone())?;
    let Some(settings) = connection_settings::effective_live(app)? else {
        return Ok(ProviderService::setup());
    };
    let profile = if settings.use_test_dc {
        "telegram-test"
    } else {
        "telegram-production"
    };
    let path = base.join(profile);
    // Lock/migrate/validate before any native connection or resumable executor.
    // FoundationStore reuses its Arc registration for this same profile during
    // replacement; there is one lock, one validator and one cached vault key.
    let store = persistence::FoundationStore::open_with_payload_validator(
        path.clone(),
        persistence::StoreBinding {
            provider: providers::telegram::locators::telegram_provider_key(),
            profile: profile.into(),
        },
        Arc::new(providers::telegram::locators::TelegramPayloadValidator),
    )?;
    let key = secure_store::load_tdlib_database_key(&path)?;
    let gateway = live_gateway::LiveGateway::connect_with_identity_store(
        live_gateway::LiveGatewayConfig::new(
            settings.library_path,
            settings.api_id,
            settings.api_hash,
            settings.use_test_dc,
            path,
            key,
        ),
        store.clone(),
    )?;
    Ok(ProviderService::new(
        providers::telegram::application::TelegramConnection::new(gateway, store),
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let application = compatibility::commands_v2::register(tauri::Builder::default())
        .setup(|app| {
            // Failed configuration/connection keeps the setup surface reachable;
            // no placeholder account, live store or executor is manufactured.
            let root = app.path().app_local_data_dir();
            let archives = match &root {
                Ok(root) => persistence::archive::ArchiveOwner::application(root.clone()),
                Err(_) => persistence::archive::ArchiveOwner::unavailable(),
            };
            let service = root
                .map_err(|_| error::AppError::StatePersistenceFailed)
                .and_then(secure_store::bind_application_root)
                .and_then(|_| create_service(app.handle()))
                .unwrap_or_else(|error| ProviderService::failed(error::boundary_error(error)));
            app.manage(Arc::new(RuntimeState {
                service: RwLock::new(service),
                archives,
            }));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Retract");
    let shutdown_started = Arc::new(AtomicBool::new(false));
    application.run(move |app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event
            && !shutdown_started.swap(true, Ordering::AcqRel)
        {
            api.prevent_exit();
            let app = app.clone();
            let runtime = app.state::<Arc<RuntimeState>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                runtime.service.write().await.shutdown().await;
                runtime.archives.shutdown().await;
                secure_store::clear_cached_secrets();
                app.exit(code.unwrap_or(0));
            });
        }
    });
}
