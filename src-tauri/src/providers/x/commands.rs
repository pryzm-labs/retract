use crate::{
    RuntimeState,
    compatibility::model_v2::{BootstrapRequest, BootstrapResponse, Empty},
    persistence::archive::ArchiveError,
    provider_service::{decode, encode, safe, validate_version},
    providers::archive_file::open_selected,
};
use retract_domain::{ErrorCode, SafeError};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::State;
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub(crate) struct Imports {
    closed: AtomicBool,
    gate: Arc<tokio::sync::Mutex<()>>,
    active: Mutex<Option<ActiveImport>>,
}
struct ActiveImport {
    cancel: Arc<AtomicBool>,
    picker: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Imports {
    pub(crate) fn stop(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancel();
    }
    fn cancel(&self) {
        if let Some(active) = self.active.lock().unwrap().as_mut() {
            active.cancel.store(true, Ordering::Release);
            if let Some(sender) = active.picker.take() {
                let _ = sender.send(());
            }
        }
    }
    pub(crate) async fn drain(&self) {
        drop(self.gate.lock().await);
    }
}
#[tauri::command]
pub(crate) async fn start_x_import_v2<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let req: BootstrapRequest<Empty> = decode(request)?;
    let service = runtime.service.read().await;
    service.check_optional(req.context.as_ref())?;
    let context = service.context();
    drop(service);
    let gate = runtime
        .x_imports
        .gate
        .clone()
        .try_lock_owned()
        .map_err(|_| safe(ErrorCode::PermissionChanged))?;
    let (cancel_picker, cancelled) = tokio::sync::oneshot::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut active = runtime.x_imports.active.lock().unwrap();
        if runtime.x_imports.closed.load(Ordering::Acquire) {
            return Err(safe(ErrorCode::Transient));
        }
        *active = Some(ActiveImport {
            cancel: cancel.clone(),
            picker: Some(cancel_picker),
        });
    }
    let (selection, selected) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("X archive", &["zip"])
        .pick_file(move |path| {
            let _ = selection.send(path);
        });
    let selected = match futures_util::future::select(selected, cancelled).await {
        futures_util::future::Either::Left((Ok(value), _)) => value,
        _ => None,
    };
    // Shutdown can hold the service write lease while draining the import gate.
    // A queued switch also invalidates the picker result before any import.
    let service = runtime
        .service
        .try_read()
        .map_err(|_| safe(ErrorCode::StaleContext))?;
    service.check_optional(context.as_ref())?;
    if service.context() != context {
        return Err(safe(ErrorCode::StaleContext));
    }
    drop(service);
    let archive = runtime.archives.clone();
    let scope = tokio::task::spawn_blocking(move || {
        let _gate = gate;
        let Some(path) = selected.and_then(|p| p.into_path().ok()) else {
            return Ok(None);
        };
        let file = open_selected(&path).map_err(|_| ArchiveError::InvalidRecord)?;
        match super::import_file(file, &archive, &cancel) {
            Err(ArchiveError::Cancelled) => Ok(None),
            result => result.map(Some),
        }
    })
    .await
    .map_err(|_| safe(ErrorCode::Transient))?
    .map_err(|error| {
        safe(match error {
            ArchiveError::InvalidRecord
            | ArchiveError::LimitExceeded
            | ArchiveError::ConflictingObservation => ErrorCode::InvalidArchive,
            ArchiveError::UnavailableKey => ErrorCode::AuthenticationRequired,
            _ => ErrorCode::Transient,
        })
    })?;
    let service = runtime.service.read().await;
    if service.context() != context {
        return Err(safe(ErrorCode::StaleContext));
    }
    encode(BootstrapResponse {
        contract_version: 2,
        context,
        payload: json!({"scope":scope}),
    })
}
#[tauri::command]
pub(crate) async fn cancel_x_import_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let req: BootstrapRequest<Empty> = decode(request)?;
    let service = runtime.service.read().await;
    service.check_optional(req.context.as_ref())?;
    runtime.x_imports.cancel();
    encode(BootstrapResponse {
        contract_version: 2,
        context: service.context(),
        payload: json!({}),
    })
}
