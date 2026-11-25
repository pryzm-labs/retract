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
    import_with_picker(&runtime, request, |selection| {
        app.dialog()
            .file()
            .add_filter("X archive", &["zip"])
            .pick_file(move |path| {
                let _ = selection.send(path.and_then(|p| p.into_path().ok()));
            });
    })
    .await
}

async fn import_with_picker(
    runtime: &RuntimeState,
    request: Value,
    pick: impl FnOnce(tokio::sync::oneshot::Sender<Option<std::path::PathBuf>>),
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let req: BootstrapRequest<Empty> = decode(request)?;
    let context = req.context;
    let original = {
        let service = runtime.service.read().await;
        service.check_optional(context.as_ref())?;
        service.clone()
    };
    let check_current = |service: &Arc<_>| {
        if !Arc::ptr_eq(&original, service) {
            return Err(safe(ErrorCode::StaleContext));
        }
        service.check_optional(context.as_ref())
    };
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
    pick(selection);
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
    check_current(&service)?;
    let archive = runtime.archives.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _gate = gate;
        let Some(path) = selected else {
            return Ok(None);
        };
        let file = open_selected(&path).map_err(|_| ArchiveError::InvalidRecord)?;
        match super::import_file(file, &archive, &cancel) {
            Err(ArchiveError::Cancelled) => Ok(None),
            result => result.map(Some),
        }
    });
    drop(service);
    let result = worker.await;
    let service = runtime.service.read().await;
    check_current(&service)?;
    let scope = result
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

#[cfg(test)]
mod picker_tests {
    use super::*;
    use crate::{persistence::archive::ArchiveOwner, provider_service::ProviderService};
    use std::time::Duration;

    fn setup() -> Arc<RuntimeState> {
        Arc::new(RuntimeState::with_archives(
            ProviderService::setup(),
            ArchiveOwner::unavailable(),
        ))
    }

    fn request() -> Value {
        json!({"contractVersion":2, "context":null, "payload":{}})
    }

    #[test]
    fn x_picker_rejects_replacement_setup_service_before_opening_file() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = setup();
        let dir = tempfile::tempdir().unwrap();
        let result = executor.block_on(import_with_picker(&state, request(), |selected| {
            *state.service.try_write().unwrap() = ProviderService::setup();
            selected
                .send(Some(dir.path().join("unavailable.zip")))
                .unwrap();
        }));
        assert_eq!(result.unwrap_err().code, ErrorCode::StaleContext);
    }

    #[test]
    fn x_picker_rechecks_setup_service_after_releasing_lease_for_dispatched_work() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = setup();
        executor.block_on(async {
            // Queue the import worker behind an occupied blocking thread.
            let (release, hold) = std::sync::mpsc::channel();
            let blocker =
                tokio::task::spawn_blocking(move || hold.recv_timeout(Duration::from_secs(3)));
            let (picked, entered) = tokio::sync::oneshot::channel();
            let importing = state.clone();
            let import = tokio::spawn(async move {
                import_with_picker(&importing, request(), |selected| {
                    selected.send(None).unwrap();
                    picked.send(()).unwrap();
                })
                .await
            });
            entered.await.unwrap();
            *state
                .service
                .try_write()
                .expect("lease released after dispatch") = ProviderService::setup();
            release.send(()).unwrap();
            blocker.await.unwrap().unwrap();
            assert_eq!(
                import.await.unwrap().unwrap_err().code,
                ErrorCode::StaleContext
            );
        });
    }

    #[test]
    fn x_picker_releases_import_gate_on_cancel_and_shutdown() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async {
            for shutdown in [false, true] {
                let state = setup();
                let importing = state.clone();
                let (opened, picker) = tokio::sync::oneshot::channel();
                let import = tokio::spawn(async move {
                    import_with_picker(&importing, request(), |selection| {
                        opened.send(selection).unwrap();
                    })
                    .await
                });
                let native_picker = picker.await.unwrap();
                if shutdown {
                    tokio::time::timeout(Duration::from_secs(3), state.shutdown(|| {}))
                        .await
                        .expect("shutdown drained the open picker");
                    assert!(import.await.unwrap().is_err());
                } else {
                    state.x_imports.cancel();
                    let result = import.await.unwrap().unwrap();
                    assert_eq!(result["payload"], json!({"scope":null}));
                }
                assert!(native_picker.send(None).is_err());
                assert!(state.x_imports.gate.try_lock().is_ok());
            }
        });
    }
}
