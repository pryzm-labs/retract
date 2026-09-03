use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use serde::Serialize;
use zeroize::Zeroizing;

use super::{
    migration::migrate_legacy,
    model::{
        FoundationState, LegacyStoreFormat, ProviderPayloadValidator, ProviderValidationPolicyKey,
        RejectProviderPayloads, StoreBinding,
    },
};
use crate::{
    error::AppError,
    secure_store::{
        KEY_LENGTH, LegacyCipherFormat, decode_legacy_state, decrypt_authenticated,
        encrypt_authenticated, load_job_store_key, write_private,
    },
};

const V3_MAGIC: &[u8; 7] = b"RTRCT03";
const ACTIVE_FILE: &str = "jobs.enc";
const BACKUP_FILE: &str = "jobs.pre-provider.enc";
const CANDIDATE_FILE: &str = "jobs.enc.tmp";
const LOCK_FILE: &str = "jobs.lock";

static OPEN_STORES: OnceLock<Mutex<HashMap<PathBuf, Weak<FoundationStore>>>> = OnceLock::new();

pub(super) trait StoreIo: Send + Sync {
    fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError>;
    fn replace(&self, from: &Path, to: &Path) -> Result<(), AppError>;
    fn sync_directory(&self, path: &Path) -> Result<(), AppError>;
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RealStoreIo;

impl StoreIo for RealStoreIo {
    fn read_candidate(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        Ok(fs::read(path)?)
    }

    fn replace(&self, from: &Path, to: &Path) -> Result<(), AppError> {
        fs::rename(from, to)?;
        Ok(())
    }

    fn sync_directory(&self, path: &Path) -> Result<(), AppError> {
        File::open(path)?.sync_all()?;
        Ok(())
    }
}

struct RuntimeState {
    committed: FoundationState,
    reload_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidationPolicy {
    Rejecting,
    Adapter(ProviderValidationPolicyKey),
}

pub struct FoundationStore {
    key: Zeroizing<[u8; KEY_LENGTH]>,
    profile_dir: PathBuf,
    active_path: PathBuf,
    binding: StoreBinding,
    validation_policy: ValidationPolicy,
    payload_validator: Arc<dyn ProviderPayloadValidator>,
    state: Mutex<RuntimeState>,
    io: Arc<dyn StoreIo>,
    _profile_lock: File,
}

impl std::fmt::Debug for FoundationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FoundationStore")
            .field("profile_dir", &self.profile_dir)
            .field("binding", &self.binding)
            .field("validation_policy", &self.validation_policy)
            .finish_non_exhaustive()
    }
}

impl FoundationStore {
    /// Opens the profile's only writer. The profile lock is acquired before the
    /// existing cached job-store key is requested.
    pub fn open(profile: PathBuf, binding: StoreBinding) -> Result<Arc<Self>, AppError> {
        Self::open_registered(
            profile,
            binding,
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
            load_job_store_key,
        )
    }

    /// Opens a profile with the owning adapter's typed provider-payload
    /// validator. Provider-backed state cannot be loaded or committed without
    /// passing this boundary.
    pub fn open_with_payload_validator(
        profile: PathBuf,
        binding: StoreBinding,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
    ) -> Result<Arc<Self>, AppError> {
        let validation_policy =
            ValidationPolicy::Adapter(payload_validator.validation_policy_key());
        Self::open_registered(
            profile,
            binding,
            validation_policy,
            payload_validator,
            load_job_store_key,
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with_test_key(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
    ) -> Result<Arc<Self>, AppError> {
        Self::open_registered(
            profile,
            binding,
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
            move |_| Ok(key),
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with_test_key_loader(
        profile: PathBuf,
        binding: StoreBinding,
        load_key: impl FnOnce(&Path) -> Result<[u8; KEY_LENGTH], AppError>,
    ) -> Result<Arc<Self>, AppError> {
        Self::open_registered(
            profile,
            binding,
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
            load_key,
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with_test_key_and_payload_validator(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
        payload_validator: Arc<dyn ProviderPayloadValidator>,
    ) -> Result<Arc<Self>, AppError> {
        let validation_policy =
            ValidationPolicy::Adapter(payload_validator.validation_policy_key());
        Self::open_registered(
            profile,
            binding,
            validation_policy,
            payload_validator,
            move |_| Ok(key),
        )
    }

    #[cfg(test)]
    pub(crate) fn open_with_test_key_loader_and_payload_validator(
        profile: PathBuf,
        binding: StoreBinding,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
        load_key: impl FnOnce(&Path) -> Result<[u8; KEY_LENGTH], AppError>,
    ) -> Result<Arc<Self>, AppError> {
        let validation_policy =
            ValidationPolicy::Adapter(payload_validator.validation_policy_key());
        Self::open_registered(
            profile,
            binding,
            validation_policy,
            payload_validator,
            load_key,
        )
    }

    #[cfg(test)]
    pub(crate) fn open_independent_with_test_key(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
    ) -> Result<Arc<Self>, AppError> {
        Self::open_independent(
            profile,
            binding,
            key,
            Arc::new(RealStoreIo),
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
        )
    }

    #[cfg(test)]
    pub(crate) fn open_independent_with_test_key_loader(
        profile: PathBuf,
        binding: StoreBinding,
        load_key: impl FnOnce(&Path) -> Result<[u8; KEY_LENGTH], AppError>,
    ) -> Result<Arc<Self>, AppError> {
        binding.validate()?;
        let profile = prepare_profile(&profile)?;
        let profile_lock = acquire_profile_lock(&profile)?;
        let key = load_key(&profile)?;
        Self::build(
            profile,
            binding,
            key,
            Arc::new(RealStoreIo),
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
            profile_lock,
        )
    }

    #[cfg(test)]
    pub(super) fn open_with_test_key_and_io(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
        io: Arc<dyn StoreIo>,
    ) -> Result<Arc<Self>, AppError> {
        Self::open_independent(
            profile,
            binding,
            key,
            io,
            ValidationPolicy::Rejecting,
            Arc::new(RejectProviderPayloads),
        )
    }

    #[cfg(test)]
    pub(super) fn open_with_test_key_and_io_and_payload_validator(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
        io: Arc<dyn StoreIo>,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
    ) -> Result<Arc<Self>, AppError> {
        let validation_policy =
            ValidationPolicy::Adapter(payload_validator.validation_policy_key());
        Self::open_independent(
            profile,
            binding,
            key,
            io,
            validation_policy,
            payload_validator,
        )
    }

    fn open_registered(
        profile: PathBuf,
        binding: StoreBinding,
        validation_policy: ValidationPolicy,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
        load_key: impl FnOnce(&Path) -> Result<[u8; KEY_LENGTH], AppError>,
    ) -> Result<Arc<Self>, AppError> {
        binding.validate()?;
        let profile = prepare_profile(&profile)?;
        let registry = OPEN_STORES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut registry = registry.lock().map_err(|_| AppError::StateUnavailable)?;
        if let Some(existing) = registry.get(&profile).and_then(Weak::upgrade) {
            if existing.binding != binding || existing.validation_policy != validation_policy {
                return Err(AppError::ProfileInUse);
            }
            return Ok(existing);
        }

        let profile_lock = acquire_profile_lock(&profile)?;
        let key = load_key(&profile)?;
        let store = Self::build(
            profile.clone(),
            binding,
            key,
            Arc::new(RealStoreIo),
            validation_policy,
            payload_validator,
            profile_lock,
        )?;
        registry.insert(profile, Arc::downgrade(&store));
        Ok(store)
    }

    #[cfg(test)]
    fn open_independent(
        profile: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
        io: Arc<dyn StoreIo>,
        validation_policy: ValidationPolicy,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
    ) -> Result<Arc<Self>, AppError> {
        binding.validate()?;
        let profile = prepare_profile(&profile)?;
        let profile_lock = acquire_profile_lock(&profile)?;
        Self::build(
            profile,
            binding,
            key,
            io,
            validation_policy,
            payload_validator,
            profile_lock,
        )
    }

    fn build(
        profile_dir: PathBuf,
        binding: StoreBinding,
        key: [u8; KEY_LENGTH],
        io: Arc<dyn StoreIo>,
        validation_policy: ValidationPolicy,
        payload_validator: Arc<dyn ProviderPayloadValidator>,
        profile_lock: File,
    ) -> Result<Arc<Self>, AppError> {
        let active_path = profile_dir.join(ACTIVE_FILE);
        let committed = initialize_state(
            &profile_dir,
            &active_path,
            &binding,
            &key,
            io.as_ref(),
            payload_validator.as_ref(),
        )?;
        Ok(Arc::new(Self {
            key: Zeroizing::new(key),
            profile_dir,
            active_path,
            binding,
            validation_policy,
            payload_validator,
            state: Mutex::new(RuntimeState {
                committed,
                reload_required: false,
            }),
            io,
            _profile_lock: profile_lock,
        }))
    }

    pub fn snapshot(&self) -> Result<FoundationState, AppError> {
        let mut state = self.state.lock().map_err(|_| AppError::StateUnavailable)?;
        self.reload_if_required(&mut state)?;
        Ok(state.committed.clone())
    }

    pub fn transaction<T>(
        &self,
        change: impl FnOnce(&mut FoundationState) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut state = self.state.lock().map_err(|_| AppError::StateUnavailable)?;
        self.reload_if_required(&mut state)?;
        let mut candidate = state.committed.clone();
        let output = change(&mut candidate)?;
        if candidate.migration != state.committed.migration
            || candidate.legacy_history != state.committed.legacy_history
        {
            return Err(AppError::SecureStore(
                "migration provenance and legacy history are immutable".into(),
            ));
        }
        candidate
            .validate_with_provider_payloads(&self.binding, self.payload_validator.as_ref())?;
        if let Err(failure) = durable_save(
            &self.profile_dir,
            &self.active_path,
            &self.binding,
            &self.key,
            &candidate,
            self.io.as_ref(),
            self.payload_validator.as_ref(),
        ) {
            state.reload_required = failure.replaced;
            return Err(AppError::StatePersistenceFailed);
        }
        state.committed = candidate;
        Ok(output)
    }

    fn reload_if_required(&self, state: &mut RuntimeState) -> Result<(), AppError> {
        if !state.reload_required {
            return Ok(());
        }
        let bytes = fs::read(&self.active_path).map_err(|_| AppError::StatePersistenceFailed)?;
        let committed = decode_v3(
            &bytes,
            &self.key,
            &self.binding,
            self.payload_validator.as_ref(),
        )
        .map_err(|_| AppError::StatePersistenceFailed)?;
        state.committed = committed;
        state.reload_required = false;
        Ok(())
    }
}

fn prepare_profile(profile: &Path) -> Result<PathBuf, AppError> {
    fs::create_dir_all(profile)?;
    Ok(fs::canonicalize(profile)?)
}

fn acquire_profile_lock(profile: &Path) -> Result<File, AppError> {
    let path = profile.join(LOCK_FILE);
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(AppError::ProfileInUse),
        Err(TryLockError::Error(error)) => Err(AppError::SecureStore(format!(
            "profile lock failed: {error}"
        ))),
    }
}

fn initialize_state(
    profile_dir: &Path,
    active_path: &Path,
    binding: &StoreBinding,
    key: &[u8; KEY_LENGTH],
    io: &dyn StoreIo,
    payload_validator: &dyn ProviderPayloadValidator,
) -> Result<FoundationState, AppError> {
    match fs::read(active_path) {
        Ok(bytes) if bytes.starts_with(V3_MAGIC) => {
            decode_v3(&bytes, key, binding, payload_validator)
        }
        Ok(bytes) => {
            let (legacy, format) = decode_legacy_state(&bytes, key, binding.profile.as_bytes())?;
            preserve_backup(profile_dir, &bytes, io)?;
            let format = match format {
                LegacyCipherFormat::Rtrct01 => LegacyStoreFormat::Rtrct01,
                LegacyCipherFormat::Rtrct02 => LegacyStoreFormat::Rtrct02,
            };
            let state = migrate_legacy(legacy, binding.clone(), format, &bytes)?;
            durable_save(
                profile_dir,
                active_path,
                binding,
                key,
                &state,
                io,
                payload_validator,
            )
            .map_err(|failure| failure.error)?;
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if profile_dir.join(BACKUP_FILE).exists() || profile_dir.join(CANDIDATE_FILE).exists() {
                return Err(AppError::SecureStore(
                    "migration evidence exists without an active job store".into(),
                ));
            }
            let state = FoundationState::empty(binding.clone());
            state.validate_with_provider_payloads(binding, payload_validator)?;
            durable_save(
                profile_dir,
                active_path,
                binding,
                key,
                &state,
                io,
                payload_validator,
            )
            .map_err(|failure| failure.error)?;
            Ok(state)
        }
        Err(error) => Err(error.into()),
    }
}

fn preserve_backup(profile_dir: &Path, source: &[u8], io: &dyn StoreIo) -> Result<(), AppError> {
    let backup_path = profile_dir.join(BACKUP_FILE);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&backup_path) {
        Ok(mut backup) => {
            #[cfg(unix)]
            backup.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            backup.write_all(source)?;
            backup.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(&backup_path)? != source {
                return Err(AppError::SecureStore(
                    "legacy backup does not match the authenticated migration source".into(),
                ));
            }
            #[cfg(unix)]
            fs::set_permissions(
                &backup_path,
                std::os::unix::fs::PermissionsExt::from_mode(0o600),
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    io.sync_directory(profile_dir)
}

struct CommitFailure {
    error: AppError,
    replaced: bool,
}

fn durable_save(
    profile_dir: &Path,
    active_path: &Path,
    binding: &StoreBinding,
    key: &[u8; KEY_LENGTH],
    candidate: &FoundationState,
    io: &dyn StoreIo,
    payload_validator: &dyn ProviderPayloadValidator,
) -> Result<(), CommitFailure> {
    let result = (|| {
        let plaintext = serde_json::to_vec(candidate)
            .map_err(|error| AppError::SecureStore(error.to_string()))?;
        let payload = encrypt_authenticated(V3_MAGIC, key, &store_aad(binding)?, &plaintext)?;
        let candidate_path = profile_dir.join(CANDIDATE_FILE);
        write_private(&candidate_path, &payload)?;
        let actual_bytes = io.read_candidate(&candidate_path)?;
        let actual = decode_v3(&actual_bytes, key, binding, payload_validator)?;
        if &actual != candidate {
            return Err(AppError::SecureStore(
                "foundation candidate verification failed".into(),
            ));
        }
        io.replace(&candidate_path, active_path)?;
        Ok(())
    })();
    if let Err(error) = result {
        return Err(CommitFailure {
            error,
            replaced: false,
        });
    }
    if let Err(error) = io.sync_directory(profile_dir) {
        return Err(CommitFailure {
            error,
            replaced: true,
        });
    }
    Ok(())
}

fn decode_v3(
    bytes: &[u8],
    key: &[u8; KEY_LENGTH],
    binding: &StoreBinding,
    payload_validator: &dyn ProviderPayloadValidator,
) -> Result<FoundationState, AppError> {
    let plaintext = decrypt_authenticated(bytes, V3_MAGIC, key, &store_aad(binding)?)?;
    let state: FoundationState = serde_json::from_slice(&plaintext)
        .map_err(|_| AppError::SecureStore("foundation state is malformed".into()))?;
    state.validate_with_provider_payloads(binding, payload_validator)?;
    Ok(state)
}

pub(super) fn store_aad(binding: &StoreBinding) -> Result<Vec<u8>, AppError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StoreAad<'a> {
        format: &'static str,
        schema: u16,
        provider: &'a retract_domain::ProviderKey,
        profile: &'a str,
        scope: &'static str,
    }
    serde_json::to_vec(&StoreAad {
        format: "RTRCT03",
        schema: super::model::FOUNDATION_SCHEMA_VERSION,
        provider: &binding.provider,
        profile: &binding.profile,
        scope: "profile",
    })
    .map_err(|error| AppError::SecureStore(error.to_string()))
}
