use super::{
    AppError,
    vault::{VAULT_ACCOUNT, VaultCache, VaultIo},
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

const V1: &[u8] = b"RTRCTV1\x010123456789abcdef0123456789abcdef";

#[derive(Clone)]
struct InjectedIo(Arc<Mutex<(Vec<u8>, usize)>>);
impl Default for InjectedIo {
    fn default() -> Self {
        Self(Arc::new(Mutex::new((V1.to_vec(), 0))))
    }
}
impl VaultIo for InjectedIo {
    fn read(&mut self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let mut state = self.0.lock().unwrap();
        state.1 += 1;
        Ok((account == VAULT_ACCOUNT).then(|| Zeroizing::new(state.0.clone())))
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        let mut state = self.0.lock().unwrap();
        state.1 += 1;
        state.0 = bytes.to_vec();
        Ok(())
    }
}
fn bound(root: PathBuf) -> VaultCache {
    let cache = VaultCache::application();
    cache.bind_root(root).unwrap();
    cache
}

#[test]
fn lease_settings_first_excludes_stale_writer_and_preserves_new_content_key() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let first = bound(root.clone());
    let second = bound(root);
    let mut io = InjectedIo::default();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    assert!(first.api_hash(&mut io).unwrap().is_some());
    let calls = io.0.lock().unwrap().1;
    assert!(matches!(
        second.save_api_hash(&mut io, "abcdef0123456789abcdef0123456789"),
        Err(AppError::ProfileInUse)
    ));
    assert_eq!(io.0.lock().unwrap().1, calls);
    let key = first.archive_key(&mut io).unwrap();
    assert_eq!(
        first.named_key(&mut io, "tdlib-database").unwrap().len(),
        32
    );
    first.clear();
    let calls = io.0.lock().unwrap().1;
    assert!(first.api_hash(&mut io).is_err());
    assert!(first.archive_key(&mut io).is_err());
    assert_eq!(io.0.lock().unwrap().1, calls);
    second
        .save_api_hash(&mut io, "abcdef0123456789abcdef0123456789")
        .unwrap();
    assert_eq!(second.archive_key(&mut io).unwrap(), key);
    assert!(io.0.lock().unwrap().0.starts_with(b"RTRCTV2"));
}

#[test]
fn lease_binding_is_required_before_any_application_vault_io() {
    let cache = VaultCache::application();
    let mut io = InjectedIo::default();
    assert!(cache.api_hash(&mut io).is_err());
    assert_eq!(io.0.lock().unwrap().1, 0);
}

#[test]
fn lease_archive_first_cached_settings_keep_ownership_until_clear() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let cache = bound(root.clone());
    let mut io = InjectedIo::default();
    let key = cache.archive_key(&mut io).unwrap();
    let calls = io.0.lock().unwrap().1;
    assert!(cache.api_hash(&mut io).unwrap().is_some());
    assert_eq!(io.0.lock().unwrap().1, calls);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "secure_store::vault_lock_tests::lease_child_process",
            "--nocapture",
        ])
        .env("RETRACT_SYNTHETIC_LEASE_ROOT", &root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(cache.archive_key(&mut io).unwrap(), key);
    cache.clear();
    assert!(bound(root).api_hash(&mut io).unwrap().is_some());
}

#[test]
fn lease_child_process() {
    let Some(root) = std::env::var_os("RETRACT_SYNTHETIC_LEASE_ROOT") else {
        return;
    };
    let cache = bound(root.into());
    let mut io = InjectedIo::default();
    assert!(matches!(
        cache.archive_key(&mut io),
        Err(AppError::ProfileInUse)
    ));
    assert_eq!(io.0.lock().unwrap().1, 0);
}

#[test]
fn lease_rejects_unsafe_artifacts_without_credential_io() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for kind in ["directory", "symlink", "permissions", "hardlink"] {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let lock = root.join("credentials.lock");
        match kind {
            "directory" => fs::create_dir(&lock).unwrap(),
            "symlink" => symlink(root.join("absent"), &lock).unwrap(),
            "hardlink" => {
                let other = root.join("other");
                fs::write(&other, b"").unwrap();
                fs::set_permissions(&other, fs::Permissions::from_mode(0o600)).unwrap();
                fs::hard_link(other, &lock).unwrap();
            }
            _ => {
                fs::write(&lock, b"").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        let mut io = InjectedIo::default();
        assert!(bound(root).api_hash(&mut io).is_err());
        assert_eq!(io.0.lock().unwrap().1, 0);
    }
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let alias = root.join("alias");
    symlink(&root, &alias).unwrap();
    let mut io = InjectedIo::default();
    assert!(bound(alias).api_hash(&mut io).is_err());
    assert_eq!(io.0.lock().unwrap().1, 0);
    let cache = bound(root.clone());
    cache.api_hash(&mut io).unwrap();
    assert_eq!(
        fs::metadata(root.join("credentials.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

pub(crate) struct InjectedApplicationVault {
    cache: VaultCache,
    io: InjectedIo,
}
impl InjectedApplicationVault {
    pub(crate) fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            cache: bound(root),
            io: InjectedIo::default(),
        })
    }
    pub(crate) fn key(&self) -> Result<crate::persistence::archive::ArchiveKey, AppError> {
        self.cache
            .archive_key(&mut self.io.clone())
            .map(crate::persistence::archive::ArchiveKey::new)
    }
    pub(crate) fn settings(&self) -> Result<(), AppError> {
        self.cache.api_hash(&mut self.io.clone()).map(|_| ())
    }
    pub(crate) fn clear(&self) {
        self.cache.clear();
    }
    pub(crate) fn calls(&self) -> usize {
        self.io.0.lock().unwrap().1
    }
}
