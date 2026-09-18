use retract_domain::{ErrorCode, SafeError, Scope};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{fs::File, sync::Arc};
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

use super::{
    application::build_connection,
    browser::{BrowserFamily, BrowserTokenCapture, CaptureCancellation},
    locators::DiscordUserLocator,
    progress::DiscordImportProgress,
};
use crate::{
    RuntimeState,
    compatibility::model_v2::{BootstrapRequest, BootstrapResponse, Empty},
    persistence::archive::{ArchiveSourceEntry, ImportFailureCode, ImportPhase, ImportWarning},
    provider_service::{decode, encode, safe, validate_version},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscordSourceView {
    scope: Scope,
    account_label: String,
    username: Option<String>,
    imported_at: Option<chrono::DateTime<chrono::Utc>>,
    warning_count: usize,
}

fn source_view(entry: &ArchiveSourceEntry) -> DiscordSourceView {
    DiscordSourceView {
        scope: entry.source.scope(),
        account_label: entry.account.display_name.clone(),
        username: entry.account.username.clone(),
        imported_at: entry.source.imported_at,
        warning_count: entry.source.warnings.len(),
    }
}

async fn ready_entries(
    runtime: &RuntimeState,
) -> Result<
    (
        Arc<crate::persistence::archive::ArchiveService>,
        Vec<ArchiveSourceEntry>,
    ),
    SafeError,
> {
    let archives = runtime.archives.open().await.map_err(archive_error)?;
    let entries = archives.ready_sources().await.map_err(archive_error)?;
    Ok((archives, entries))
}

fn archive_error(error: crate::persistence::archive::ArchiveError) -> SafeError {
    safe(match error {
        crate::persistence::archive::ArchiveError::UnavailableKey => {
            ErrorCode::AuthenticationRequired
        }
        crate::persistence::archive::ArchiveError::InvalidRecord
        | crate::persistence::archive::ArchiveError::InvalidStore => ErrorCode::InvalidArchive,
        crate::persistence::archive::ArchiveError::ScopeMismatch => ErrorCode::NotFound,
        _ => ErrorCode::Transient,
    })
}

#[tauri::command]
pub(crate) async fn list_sources_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    let (_, entries) = ready_entries(&runtime).await?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: entries.iter().map(source_view).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectArchive {
    scope: Scope,
}

#[tauri::command]
pub(crate) async fn select_archive_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<SelectArchive> = decode(request)?;
    let mut current = runtime.service.write().await;
    current.check_optional(request.context.as_ref())?;
    if current.has_workers().await {
        return Err(safe(ErrorCode::PermissionChanged));
    }
    let (archives, entries) = ready_entries(&runtime).await?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.source.scope() == request.payload.scope)
        .ok_or_else(|| safe(ErrorCode::NotFound))?;
    current.shutdown().await;
    *current = crate::provider_service::ProviderService::new(build_connection(
        &runtime.application_root,
        entry,
        archives,
        runtime.discord_session.clone(),
    )?);
    current
        .bootstrap(
            serde_json::json!({"contractVersion":2,"context":current.context(),"payload":{}}),
        )
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportView {
    active: bool,
    progress: Option<DiscordImportProgress>,
    sources: Vec<DiscordSourceView>,
    import_scope: Option<Scope>,
    retry_available: bool,
    failure_code: Option<ImportFailureCode>,
    warning_details: Vec<ImportWarning>,
}

async fn import_view(runtime: &RuntimeState) -> Result<ImportView, SafeError> {
    let (_, entries) = ready_entries(runtime).await?;
    let progress = runtime.discord_imports.active_progress();
    let checkpoint = runtime.discord_imports.active_checkpoint();
    let active = progress.as_ref().is_some_and(|progress| {
        !matches!(
            progress.phase,
            super::progress::DiscordImportPhase::Ready
                | super::progress::DiscordImportPhase::Cancelled
                | super::progress::DiscordImportPhase::Failed
        )
    });
    Ok(ImportView {
        active,
        progress,
        sources: entries.iter().map(source_view).collect(),
        import_scope: checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.scope.clone()),
        retry_available: checkpoint.as_ref().is_some_and(|checkpoint| {
            matches!(
                checkpoint.progress.phase,
                ImportPhase::Interrupted | ImportPhase::Cancelled | ImportPhase::Failed
            )
        }),
        failure_code: checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.failure_code),
        warning_details: checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.warnings.clone())
            .unwrap_or_default(),
    })
}

#[tauri::command]
pub(crate) async fn start_discord_import_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    let selected = app
        .dialog()
        .file()
        .add_filter("Discord data package", &["zip"])
        .blocking_pick_file();
    let Some(path) = selected.and_then(|path| path.into_path().ok()) else {
        return encode(BootstrapResponse {
            contract_version: 2,
            context: runtime.service.read().await.context(),
            payload: import_view(&runtime).await?,
        });
    };
    let file = open_selected(&path).map_err(|_| safe(ErrorCode::InvalidArchive))?;
    runtime
        .discord_imports
        .start_or_retry(file)
        .await
        .map_err(import_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: import_view(&runtime).await?,
    })
}

#[tauri::command]
pub(crate) async fn get_discord_import_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: import_view(&runtime).await?,
    })
}

#[tauri::command]
pub(crate) async fn retry_discord_import_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    let selected = app
        .dialog()
        .file()
        .add_filter("Discord data package", &["zip"])
        .blocking_pick_file();
    let Some(path) = selected.and_then(|path| path.into_path().ok()) else {
        return encode(BootstrapResponse {
            contract_version: 2,
            context: runtime.service.read().await.context(),
            payload: import_view(&runtime).await?,
        });
    };
    let file = open_selected(&path).map_err(|_| safe(ErrorCode::InvalidArchive))?;
    runtime
        .discord_imports
        .retry_active(file)
        .await
        .map_err(import_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: import_view(&runtime).await?,
    })
}

#[tauri::command]
pub(crate) async fn cancel_discord_import_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    runtime.discord_imports.cancel_active();
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: import_view(&runtime).await?,
    })
}

#[tauri::command]
pub(crate) async fn get_discord_session_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    let owner = expected_owner(&runtime, request.context.as_ref()).await?;
    let status = runtime
        .discord_session
        .load_remembered(&owner)
        .await
        .map_err(crate::error::boundary_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: status,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserView {
    id: String,
    display_name: String,
    family: BrowserFamily,
}

#[tauri::command]
pub(crate) async fn discover_discord_browsers_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    let browsers = BrowserTokenCapture::discover()
        .into_iter()
        .map(|browser| BrowserView {
            id: browser.id,
            display_name: browser.display_name,
            family: browser.family,
        })
        .collect::<Vec<_>>();
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: browsers,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManualToken {
    #[serde(deserialize_with = "deserialize_secret")]
    token: Zeroizing<String>,
    remember: bool,
    risk_acknowledged: bool,
}

fn deserialize_secret<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[tauri::command]
pub(crate) async fn submit_discord_token_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<ManualToken> = decode(request)?;
    if !request.payload.risk_acknowledged {
        return Err(safe(ErrorCode::PermissionChanged));
    }
    let owner = expected_owner(&runtime, request.context.as_ref()).await?;
    let status = runtime
        .discord_session
        .submit_manual(&owner, &request.payload.token, request.payload.remember)
        .await
        .map_err(crate::error::boundary_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: status,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserAuth {
    browser_id: String,
    remember: bool,
    risk_acknowledged: bool,
}

#[tauri::command]
pub(crate) async fn start_discord_browser_auth_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<BrowserAuth> = decode(request)?;
    if !request.payload.risk_acknowledged {
        return Err(safe(ErrorCode::PermissionChanged));
    }
    let owner = expected_owner(&runtime, request.context.as_ref()).await?;
    let browser = BrowserTokenCapture::discover()
        .into_iter()
        .find(|browser| browser.id == request.payload.browser_id)
        .ok_or_else(|| safe(ErrorCode::NotFound))?;
    let cancellation = CaptureCancellation::default();
    {
        let mut active = runtime
            .discord_capture
            .lock()
            .map_err(|_| safe(ErrorCode::Transient))?;
        if active.is_some() {
            return Err(safe(ErrorCode::PermissionChanged));
        }
        *active = Some(cancellation.clone());
    }
    let result = BrowserTokenCapture::capture(
        &browser,
        &owner,
        request.payload.remember,
        &runtime.discord_session,
        &cancellation,
        |_| {},
    )
    .await;
    runtime
        .discord_capture
        .lock()
        .map_err(|_| safe(ErrorCode::Transient))?
        .take();
    let status = result.map_err(crate::error::boundary_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: status,
    })
}

#[tauri::command]
pub(crate) async fn cancel_discord_browser_auth_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    if let Some(cancellation) = runtime
        .discord_capture
        .lock()
        .map_err(|_| safe(ErrorCode::Transient))?
        .as_ref()
    {
        cancellation.cancel();
    }
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: Empty {},
    })
}

#[tauri::command]
pub(crate) async fn forget_discord_session_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    runtime
        .discord_session
        .forget()
        .map_err(crate::error::boundary_error)?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: runtime.discord_session.status(),
    })
}

async fn expected_owner(
    runtime: &RuntimeState,
    context: Option<&retract_domain::ActiveContext>,
) -> Result<String, SafeError> {
    let service = runtime.service.read().await;
    service.check_optional(context)?;
    let actual = service
        .context()
        .ok_or_else(|| safe(ErrorCode::IdentityUnavailable))?;
    if actual.scope.provider.as_str() != "discord" {
        return Err(safe(ErrorCode::ScopeMismatch));
    }
    drop(service);
    let (_, entries) = ready_entries(runtime).await?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.source.scope() == actual.scope)
        .ok_or_else(|| safe(ErrorCode::NotFound))?;
    let locator: DiscordUserLocator = serde_json::from_value(entry.account.native_identity.payload)
        .map_err(|_| safe(ErrorCode::InvalidArchive))?;
    Ok(locator.user_id)
}

fn import_error(error: super::import::DiscordImportError) -> SafeError {
    safe(match error {
        super::import::DiscordImportError::InvalidArchive
        | super::import::DiscordImportError::UnsupportedProfile
        | super::import::DiscordImportError::InputChanged
        | super::import::DiscordImportError::RetryMismatch => ErrorCode::InvalidArchive,
        super::import::DiscordImportError::UnavailableKey => ErrorCode::AuthenticationRequired,
        super::import::DiscordImportError::Busy => ErrorCode::PermissionChanged,
        _ => ErrorCode::Transient,
    })
}

#[cfg(unix)]
fn open_selected(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}
#[cfg(not(unix))]
fn open_selected(path: &std::path::Path) -> std::io::Result<File> {
    File::open(path)
}
