use crate::providers::archive_commands::{SourceView, ready_entries, source_view};
use crate::providers::archive_file::open_selected;
use retract_domain::{ErrorCode, SafeError, Scope};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

use super::{
    browser::{BrowserFamily, BrowserTokenCapture, CaptureCancellation},
    locators::DiscordUserLocator,
    progress::DiscordImportProgress,
};
use crate::{
    RuntimeState,
    compatibility::model_v2::{BootstrapRequest, BootstrapResponse, Empty},
    persistence::archive::{ImportFailureCode, ImportPhase, ImportWarning},
    provider_service::{decode, encode, safe, validate_version},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportView {
    active: bool,
    progress: Option<DiscordImportProgress>,
    sources: Vec<SourceView>,
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
    import_with_picker(&runtime, request, ImportAction::Start, || {
        pick_archive(&app)
    })
    .await
}

enum ImportAction {
    Start,
    Retry,
}

fn pick_archive<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<std::path::PathBuf> {
    app.dialog()
        .file()
        .add_filter("Discord data package", &["zip"])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
}

async fn import_with_picker(
    runtime: &RuntimeState,
    request: Value,
    action: ImportAction,
    pick: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    let original = {
        let service = runtime.service.read().await;
        service.check_optional(request.context.as_ref())?;
        service.clone()
    };
    let selected = pick();
    // A picker can stay open across a workspace switch, including a switch
    // between two setup services with no active context. Hold the read lease
    // through worker admission, allowing switches while the dialog is open.
    let service = runtime.service.read().await;
    let current = || service.check_optional(request.context.as_ref()).is_ok();
    if !Arc::ptr_eq(&original, &service) || !current() {
        return Err(safe(ErrorCode::StaleContext));
    }
    if let Some(path) = selected {
        let file = open_selected(&path).map_err(|_| safe(ErrorCode::InvalidArchive))?;
        match action {
            ImportAction::Start => runtime.discord_imports.start_or_retry(file, current).await,
            ImportAction::Retry => runtime.discord_imports.retry_active(file, current).await,
        }
        .map_err(import_error)?;
    }
    let payload = import_view(runtime).await?;
    if !current() {
        return Err(safe(ErrorCode::StaleContext));
    }
    encode(BootstrapResponse {
        contract_version: 2,
        context: request.context,
        payload,
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
    import_with_picker(&runtime, request, ImportAction::Retry, || {
        pick_archive(&app)
    })
    .await
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
        super::import::DiscordImportError::StaleContext => ErrorCode::StaleContext,
        super::import::DiscordImportError::InvalidArchive
        | super::import::DiscordImportError::UnsupportedProfile
        | super::import::DiscordImportError::InputChanged
        | super::import::DiscordImportError::RetryMismatch => ErrorCode::InvalidArchive,
        super::import::DiscordImportError::UnavailableKey => ErrorCode::AuthenticationRequired,
        super::import::DiscordImportError::Busy => ErrorCode::PermissionChanged,
        _ => ErrorCode::Transient,
    })
}

#[cfg(test)]
mod import_picker_tests {
    use super::*;
    use crate::{
        compatibility::fixtures::{RolloverConnection, SyntheticIo, context},
        persistence::archive::ArchiveOwner,
        provider_service::ProviderService,
    };
    use serde_json::json;

    #[test]
    fn start_and_retry_reject_generation_change_while_picker_is_open() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("selected.zip");
        std::fs::write(&selected, b"synthetic selection").unwrap();
        let original = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(original.clone()));
        let connection = RolloverConnection::new(&directory.path().join("profile"), io.clone());
        let state = RuntimeState::with_archives(
            ProviderService::new(connection),
            ArchiveOwner::unavailable(),
        );
        for action in [ImportAction::Start, ImportAction::Retry] {
            *io.active.write().unwrap() = Some(original.clone());
            let request = json!({"contractVersion":2, "context":original, "payload":{}});
            let result = executor.block_on(import_with_picker(&state, request, action, || {
                io.active
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .session_generation = uuid::Uuid::new_v4();
                Some(selected.clone())
            }));
            assert!(state.discord_imports.active_progress().is_none());
            assert_eq!(result.unwrap_err().code, ErrorCode::StaleContext);
        }
    }

    #[test]
    fn setup_import_rejects_replacement_service_with_no_context() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state =
            RuntimeState::with_archives(ProviderService::setup(), ArchiveOwner::unavailable());
        let request = json!({"contractVersion":2, "context":null, "payload":{}});
        let result = executor.block_on(import_with_picker(
            &state,
            request,
            ImportAction::Start,
            || {
                *state.service.try_write().unwrap() = ProviderService::setup();
                None
            },
        ));
        assert_eq!(result.unwrap_err().code, ErrorCode::StaleContext);
    }
}

#[cfg(all(test, unix))]
mod open_selected_tests {
    use super::open_selected;
    use std::{
        ffi::CString,
        fs::{self, OpenOptions},
        os::{fd::AsRawFd, unix::ffi::OsStrExt, unix::fs::OpenOptionsExt},
        sync::mpsc,
        thread,
        time::Duration,
    };

    #[test]
    fn selected_fifo_is_rejected_without_blocking_for_a_writer() {
        let directory = tempfile::tempdir().unwrap();
        let fifo = directory.path().join("selected.zip");
        let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

        let selected = fifo.clone();
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            sent.send(open_selected(&selected).map(|_| ())).unwrap();
        });

        match received.recv_timeout(Duration::from_millis(300)) {
            Ok(result) => assert!(result.is_err(), "a FIFO was accepted as an archive"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _writer = OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo)
                    .unwrap();
                let _ = received.recv_timeout(Duration::from_secs(1));
                worker.join().unwrap();
                panic!("opening a selected FIFO blocked before validation");
            }
            Err(error) => panic!("archive-open worker disconnected: {error}"),
        }
        worker.join().unwrap();
    }

    #[test]
    fn selected_directory_is_rejected_after_descriptor_validation() {
        let directory = tempfile::tempdir().unwrap();
        assert!(open_selected(directory.path()).is_err());
    }

    #[test]
    fn selected_regular_file_is_blocking_after_validation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selected.zip");
        fs::write(&path, b"synthetic archive").unwrap();

        let file = open_selected(&path).unwrap();
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(flags & libc::O_NONBLOCK, 0);
    }
}
