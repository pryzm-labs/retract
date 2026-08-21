mod connection_settings;
mod demo_gateway;
mod error;
mod gateway;
mod live_gateway;
mod local_auth;
mod model;
mod secure_store;
mod service;
mod tdjson;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use cleaner_domain::ChatSummary;
use demo_gateway::DemoGateway;
use error::{AppError, CommandError};
use model::{
    AppSnapshot, AuthSnapshot, AuthValueRequest, AuthorizePlanRequest, CatalogProgress,
    ExecuteRequest, JobRecord, PlanView, PrepareChatActionRequest, PrepareSelectionRequest,
    PrepareSenderActionRequest, SearchRequest, SearchResponse,
};
use service::CleanerService;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;
use uuid::Uuid;

struct RuntimeState {
    service: RwLock<Arc<CleanerService>>,
}

impl RuntimeState {
    fn new(service: Arc<CleanerService>) -> Self {
        Self {
            service: RwLock::new(service),
        }
    }

    async fn current(&self) -> Arc<CleanerService> {
        self.service.read().await.clone()
    }

    async fn replace(&self, service: Arc<CleanerService>) {
        *self.service.write().await = service;
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveConnectionSettingsResult {
    connection_settings: connection_settings::ConnectionSettingsView,
    snapshot: AppSnapshot,
}

#[tauri::command]
async fn get_snapshot(runtime: State<'_, Arc<RuntimeState>>) -> Result<AppSnapshot, CommandError> {
    let service = runtime.current().await;
    service.snapshot().await.map_err(Into::into)
}

#[tauri::command]
async fn get_bootstrap_snapshot(
    runtime: State<'_, Arc<RuntimeState>>,
) -> Result<AppSnapshot, CommandError> {
    let service = runtime.current().await;
    service.bootstrap_snapshot().await.map_err(Into::into)
}

#[tauri::command]
async fn get_auth_snapshot(
    runtime: State<'_, Arc<RuntimeState>>,
) -> Result<AuthSnapshot, CommandError> {
    let service = runtime.current().await;
    Ok(service.auth_snapshot())
}

#[tauri::command]
async fn get_catalog_progress(
    runtime: State<'_, Arc<RuntimeState>>,
) -> Result<CatalogProgress, CommandError> {
    let service = runtime.current().await;
    Ok(service.catalog_progress())
}

#[tauri::command]
async fn search_messages(
    runtime: State<'_, Arc<RuntimeState>>,
    request: SearchRequest,
) -> Result<SearchResponse, CommandError> {
    let service = runtime.current().await;
    service.search(request).await.map_err(Into::into)
}

#[tauri::command]
async fn refresh_chats(
    runtime: State<'_, Arc<RuntimeState>>,
    chat_ids: Vec<i64>,
) -> Result<Vec<ChatSummary>, CommandError> {
    let service = runtime.current().await;
    service.refresh_chats(chat_ids).await.map_err(Into::into)
}

#[tauri::command]
async fn prepare_selection(
    runtime: State<'_, Arc<RuntimeState>>,
    request: PrepareSelectionRequest,
) -> Result<PlanView, CommandError> {
    let service = runtime.current().await;
    service.prepare_selection(request).await.map_err(Into::into)
}

#[tauri::command]
async fn prepare_own_messages(
    runtime: State<'_, Arc<RuntimeState>>,
    chat_id: i64,
) -> Result<PlanView, CommandError> {
    let service = runtime.current().await;
    service
        .prepare_own_messages(chat_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn prepare_chat_action(
    runtime: State<'_, Arc<RuntimeState>>,
    request: PrepareChatActionRequest,
) -> Result<PlanView, CommandError> {
    let service = runtime.current().await;
    service
        .prepare_chat_action(request)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn prepare_sender_action(
    runtime: State<'_, Arc<RuntimeState>>,
    request: PrepareSenderActionRequest,
) -> Result<PlanView, CommandError> {
    let service = runtime.current().await;
    service
        .prepare_sender_action(request)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn request_qr_auth(runtime: State<'_, Arc<RuntimeState>>) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service.request_qr_auth().await.map_err(Into::into)
}

#[tauri::command]
async fn submit_phone(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthValueRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service
        .submit_phone(&request.value)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn submit_email_address(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthValueRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service
        .submit_email_address(&request.value)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn submit_email_code(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthValueRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service
        .submit_email_code(&request.value)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn submit_code(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthValueRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service
        .submit_code(&request.value)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn submit_password(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthValueRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service
        .submit_password(&request.value)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn start_execution(
    runtime: State<'_, Arc<RuntimeState>>,
    request: ExecuteRequest,
) -> Result<JobRecord, CommandError> {
    let service = runtime.current().await;
    service.start_execution(request).await.map_err(Into::into)
}

#[tauri::command]
async fn authorize_plan(
    runtime: State<'_, Arc<RuntimeState>>,
    request: AuthorizePlanRequest,
) -> Result<(), CommandError> {
    let service = runtime.current().await;
    service.authorize_plan(request).await.map_err(Into::into)
}

#[tauri::command]
async fn get_jobs(runtime: State<'_, Arc<RuntimeState>>) -> Result<Vec<JobRecord>, CommandError> {
    let service = runtime.current().await;
    Ok(service.jobs().await)
}

#[tauri::command]
async fn cancel_job(
    runtime: State<'_, Arc<RuntimeState>>,
    job_id: Uuid,
) -> Result<JobRecord, CommandError> {
    let service = runtime.current().await;
    service.cancel_job(job_id).await.map_err(Into::into)
}

#[tauri::command]
async fn reset_demo(runtime: State<'_, Arc<RuntimeState>>) -> Result<AppSnapshot, CommandError> {
    let service = runtime.current().await;
    service.reset_demo().await.map_err(CommandError::from)?;
    service.snapshot().await.map_err(Into::into)
}

#[tauri::command]
fn get_connection_settings(
    app: AppHandle,
) -> Result<connection_settings::ConnectionSettingsView, CommandError> {
    connection_settings::get_view(&app).map_err(Into::into)
}

#[tauri::command]
async fn save_connection_settings(
    app: AppHandle,
    runtime: State<'_, Arc<RuntimeState>>,
    request: connection_settings::SaveConnectionSettingsRequest,
) -> Result<SaveConnectionSettingsResult, CommandError> {
    let current = runtime.current().await;
    if current
        .jobs()
        .await
        .iter()
        .any(|job| !job.status.is_terminal())
    {
        return Err(CommandError::from(error::AppError::InvalidRequest(
            "wait for the active cleanup job to finish or cancel it before changing connections"
                .into(),
        )));
    }
    connection_settings::save(&app, request).map_err(CommandError::from)?;

    current.shutdown().await;
    let next = create_service(&app).map_err(CommandError::from)?;
    next.resume_incomplete().await;
    runtime.replace(Arc::clone(&next)).await;

    Ok(SaveConnectionSettingsResult {
        connection_settings: connection_settings::get_view(&app).map_err(CommandError::from)?,
        snapshot: next.snapshot().await.map_err(CommandError::from)?,
    })
}

fn create_service(app: &AppHandle) -> Result<Arc<CleanerService>, AppError> {
    let base_data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    let effective_live = match connection_settings::effective_live(app) {
        Ok(Some(settings)) => Some(Ok(settings)),
        Ok(None) => None,
        Err(error) => Some(Err(error)),
    };
    let test_dc = effective_live
        .as_ref()
        .and_then(|settings| settings.as_ref().ok())
        .is_some_and(|settings| settings.use_test_dc);
    let live_profile = base_data_dir.join(if test_dc {
        "telegram-test"
    } else {
        "telegram-production"
    });
    let live = effective_live.map(|settings| {
        settings.and_then(|settings| {
            secure_store::load_tdlib_database_key(&live_profile).and_then(|key| {
                live_gateway::LiveGateway::connect(live_gateway::LiveGatewayConfig::new(
                    settings.library_path,
                    settings.api_id,
                    settings.api_hash,
                    settings.use_test_dc,
                    live_profile.clone(),
                    key,
                ))
            })
        })
    });
    let (gateway, profile_dir, is_demo): (Arc<dyn gateway::TelegramGateway>, _, _) = match live {
        Some(Ok(gateway)) => (gateway, live_profile, false),
        Some(Err(error)) => (
            Arc::new(DemoGateway::with_reason(format!(
                "Live mode could not start: {error}. Open Settings to correct the connection. Destructive actions remain confined to fixtures."
            ))),
            base_data_dir.join("demo"),
            true,
        ),
        None => (
            Arc::new(DemoGateway::new()),
            base_data_dir.join("demo"),
            true,
        ),
    };
    let store = if is_demo {
        secure_store::SecureJobStore::open_demo(profile_dir)?
    } else {
        secure_store::SecureJobStore::open(profile_dir)?
    };
    CleanerService::new(gateway, store)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let application = tauri::Builder::default()
        .setup(|app| {
            let service = create_service(app.handle())
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            app.manage(Arc::new(RuntimeState::new(Arc::clone(&service))));
            tauri::async_runtime::spawn(async move {
                service.resume_incomplete().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_bootstrap_snapshot,
            get_auth_snapshot,
            get_catalog_progress,
            search_messages,
            refresh_chats,
            prepare_selection,
            prepare_own_messages,
            prepare_chat_action,
            prepare_sender_action,
            request_qr_auth,
            submit_phone,
            submit_email_address,
            submit_email_code,
            submit_code,
            submit_password,
            authorize_plan,
            start_execution,
            get_jobs,
            cancel_job,
            reset_demo,
            get_connection_settings,
            save_connection_settings
        ])
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
                let service = runtime.current().await;
                service.shutdown().await;
                secure_store::clear_cached_secrets();
                app.exit(code.unwrap_or(0));
            });
        }
    });
}
