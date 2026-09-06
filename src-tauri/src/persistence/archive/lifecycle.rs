//! Application-lifetime registration of lazy archive work and terminal drain.
use super::{ArchiveError, ArchiveService, ArchiveStore};
use std::sync::Arc;
use tokio::sync::Mutex;

type Opener = dyn Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync;
pub(crate) struct ArchiveOwner {
    opener: Arc<Opener>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    service: Option<Arc<ArchiveService>>,
}

impl ArchiveOwner {
    pub(crate) fn unavailable() -> Self {
        Self::with_opener(|| Err(ArchiveError::UnavailableKey))
    }
    pub(super) fn with_opener(
        opener: impl Fn() -> Result<ArchiveStore, ArchiveError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            opener: Arc::new(opener),
            state: Mutex::new(State::default()),
        }
    }

    pub(crate) fn application(root: std::path::PathBuf) -> Self {
        Self::with_opener(move || {
            #[cfg(target_os = "macos")]
            {
                let path = prepare_archive_directory(&root)?.join("content.db");
                // There are no registered archive adapters in this stage.
                ArchiveStore::open_with_key_loader(path, Default::default(), || {
                    crate::secure_store::load_archive_index_key().map_err(|error| match error {
                        crate::error::AppError::ProfileInUse => ArchiveError::StoreInUse,
                        _ => ArchiveError::UnavailableKey,
                    })
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = &root;
                Err(ArchiveError::UnavailableKey)
            }
        })
    }

    /// First-open failures are sticky to avoid repeated credential prompts/I/O.
    /// Correct the condition and restart the application; import retry and Busy
    /// backoff do not reset this application-lifetime factory.
    pub(crate) async fn open(&self) -> Result<Arc<ArchiveService>, ArchiveError> {
        let mut lifecycle = self.state.lock().await;
        if lifecycle.closed {
            return Err(ArchiveError::Cancelled);
        }
        let service = lifecycle.service.get_or_insert_with(|| {
            let factory = self.opener.clone();
            ArchiveService::start(move || factory())
        });
        service.wait_ready().await?;
        Ok(Arc::clone(service))
    }

    pub(crate) async fn shutdown(&self) {
        let mut lifecycle = self.state.lock().await;
        lifecycle.closed = true;
        if let Some(service) = lifecycle.service.as_ref() {
            service.shutdown().await;
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn prepare_archive_directory(
    root: &std::path::Path,
) -> Result<std::path::PathBuf, ArchiveError> {
    use std::{fs, io::ErrorKind, os::unix::fs::DirBuilderExt};
    // Create only beneath an existing canonical, non-writable parent; all work
    // runs on the worker. Never relax the repository's existing parent checks.
    if let Err(error) = fs::symlink_metadata(root) {
        if error.kind() != ErrorKind::NotFound {
            return Err(ArchiveError::InvalidStore);
        }
        super::store::validate_parent(root)?;
        match fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ArchiveError::StorageFailure),
        }
    }
    super::store::validate_parent(&root.join("archives"))?;
    let directory = root.join("archives");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(ArchiveError::StorageFailure),
    }
    super::store::validate_parent(&directory.join("content.db"))?;
    Ok(directory)
}
