use super::model_v2::{BootstrapRequest, BootstrapResponse, Empty};
use crate::{
    RuntimeState,
    error::boundary_error,
    provider_service::{decode, encode, safe, validate_version},
};
use retract_domain::{ErrorCode, SafeError};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;

macro_rules! active_command {
    ($name:ident, $operation:literal) => {
        #[tauri::command]
        pub(crate) async fn $name(
            runtime: State<'_, Arc<RuntimeState>>,
            request: Value,
        ) -> Result<Value, SafeError> {
            runtime
                .service
                .read()
                .await
                .active($operation, request)
                .await
        }
    };
}
active_command!(get_snapshot_v2, "snapshot");
active_command!(search_messages_v2, "search");
active_command!(refresh_chats_v2, "refresh");
active_command!(prepare_selection_v2, "prepare_selection");
active_command!(prepare_intent_v2, "prepare_intent");
active_command!(get_intents_v2, "intents");
active_command!(authorize_plan_v2, "authorize");
active_command!(start_execution_v2, "execute");
active_command!(get_jobs_v2, "jobs");
active_command!(cancel_job_v2, "cancel");

#[tauri::command]
pub(crate) async fn get_bootstrap_snapshot_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    runtime.service.read().await.bootstrap(request).await
}
#[tauri::command]
pub(crate) async fn submit_auth_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    runtime.service.read().await.auth(request, false).await
}
#[tauri::command]
pub(crate) async fn retry_identity_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    runtime
        .retry_identity(request, || crate::create_service(&app))
        .await
}

#[tauri::command]
pub(crate) async fn get_connection_settings_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    let service = runtime.service.read().await;
    service.check_optional(request.context.as_ref())?;
    let mut settings = crate::connection_settings::get_view(&app).map_err(boundary_error)?;
    if settings.configuration_error.is_some() {
        settings.configuration_error =
            Some("Connection settings could not be loaded. Review and save them again.".into());
    }
    service.check_optional(request.context.as_ref())?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: service.context(),
        payload: settings,
    })
}

#[tauri::command]
pub(crate) async fn save_connection_settings_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    runtime
        .save_settings(
            request,
            |settings| crate::connection_settings::save(&app, settings),
            || crate::create_service(&app),
        )
        .await
}

impl RuntimeState {
    /// The registered command owns the real settings/vault and connection
    /// factories; this lifecycle seam also admits synthetic I/O in tests.
    pub(crate) async fn save_settings(
        &self,
        request: Value,
        save: impl FnOnce(
            crate::connection_settings::SaveConnectionSettingsRequest,
        ) -> Result<(), crate::error::AppError>,
        create: impl FnOnce() -> Result<
            Arc<crate::provider_service::ProviderService>,
            crate::error::AppError,
        >,
    ) -> Result<Value, SafeError> {
        validate_version(&request)?;
        let request: BootstrapRequest<crate::connection_settings::SaveConnectionSettingsRequest> =
            decode(request)?;
        let mut current = self.service.write().await;
        current.check_optional(request.context.as_ref())?;
        if current.has_workers().await {
            return Err(safe(ErrorCode::PermissionChanged));
        }
        current.check_optional(request.context.as_ref())?;
        save(request.payload).map_err(boundary_error)?;
        Self::recreate(&mut current, create).await
    }

    pub(crate) async fn retry_identity(
        &self,
        raw: Value,
        create: impl FnOnce() -> Result<
            Arc<crate::provider_service::ProviderService>,
            crate::error::AppError,
        >,
    ) -> Result<Value, SafeError> {
        validate_version(&raw)?;
        let request: BootstrapRequest<Empty> = decode(raw.clone())?;
        let mut current = self.service.write().await;
        current.check_optional(request.context.as_ref())?;
        if !current.requires_recreation() {
            return current.auth(raw, true).await;
        }
        Self::recreate(&mut current, create).await
    }

    async fn recreate(
        current: &mut Arc<crate::provider_service::ProviderService>,
        create: impl FnOnce() -> Result<
            Arc<crate::provider_service::ProviderService>,
            crate::error::AppError,
        >,
    ) -> Result<Value, SafeError> {
        current.shutdown().await;
        let next = match create() {
            Ok(next) => next,
            Err(error) => {
                let diagnostic = boundary_error(error);
                *current = crate::provider_service::ProviderService::failed(diagnostic.clone());
                return Err(diagnostic);
            }
        };
        *current = next;
        current
            .bootstrap(
                serde_json::json!({"contractVersion":2,"context":current.context(),"payload":{}}),
            )
            .await
    }
}

/// Production and tests use this same registration, not a parallel test router.
pub fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        get_snapshot_v2,
        get_bootstrap_snapshot_v2,
        search_messages_v2,
        refresh_chats_v2,
        prepare_selection_v2,
        prepare_intent_v2,
        get_intents_v2,
        authorize_plan_v2,
        start_execution_v2,
        get_jobs_v2,
        cancel_job_v2,
        submit_auth_v2,
        retry_identity_v2,
        get_connection_settings_v2,
        save_connection_settings_v2
    ])
}
