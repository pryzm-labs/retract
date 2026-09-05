use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::ErrorKind,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use retract_domain::{
    AccountRecord, ConnectionState, ProviderKey, Scope, SourceRecord, SourceState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::persistence::ProviderPayloadValidator;

use super::{ArchiveError, ArchiveKey, codec::open_keyed, model, preflight, schema};

#[cfg(test)]
#[path = "test_support.rs"]
pub(in crate::persistence) mod test_support;

#[cfg(test)]
type MaintenanceHook = dyn Fn(&Connection) -> Result<(), ArchiveError> + Send + Sync;

pub(crate) struct ArchiveStore {
    // Declaration order closes the connection before releasing the file lock.
    pub(super) connection: Mutex<Connection>,
    pub(super) key: ArchiveKey,
    pub(super) path: PathBuf,
    pub(super) validators: BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
    pub(super) import_limits: model::ImportLimits,
    #[cfg(test)]
    pub(super) before_commit: Option<Box<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    pub(super) before_maintenance: Option<Box<MaintenanceHook>>,
    _process_lock: ProcessLock,
}

pub(super) struct ProcessLock(File);

impl Drop for ProcessLock {
    fn drop(&mut self) {
        // Explicit unlock releases this open-file-description lock even if a
        // concurrently spawned child briefly inherited the descriptor pre-exec.
        // ArchiveStore drops the connection before this guard.
        let _ = self.0.unlock();
    }
}

impl ArchiveStore {
    pub(crate) fn open(
        path: PathBuf,
        key: ArchiveKey,
        validators: BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
    ) -> Result<Self, ArchiveError> {
        Self::open_with_key_loader(path, validators, || Ok(key))
    }

    pub(crate) fn open_with_key_loader(
        path: PathBuf,
        validators: BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
        load: impl FnOnce() -> Result<ArchiveKey, ArchiveError>,
    ) -> Result<Self, ArchiveError> {
        validate_parent(&path)?;
        validate_artifacts(&path)?;
        let process_lock = acquire_lock(&sidecar(&path, ".lock"))?;
        super::migration::validate_active_presence(&path)?;
        preflight::validate_artifact_set(&path)?;
        let key = load()?;
        let is_new = match private_options().create_new(true).open(&path) {
            Ok(file) => {
                file.sync_all().map_err(|_| ArchiveError::StorageFailure)?;
                true
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
            Err(_) => return Err(ArchiveError::StorageFailure),
        };
        validate_artifacts(&path)?;
        if !is_new
            && fs::metadata(&path)
                .map_err(|_| ArchiveError::InvalidStore)?
                .len()
                == 0
        {
            return Err(ArchiveError::InvalidStore);
        }
        if !is_new {
            preflight::validate_existing(&path, &key, |connection| {
                schema::validate(connection)?;
                validate_registrations(connection, &validators)?;
                super::ingest_state::validate_runs(connection, &validators)
            })?;
        }
        let mut connection = open_keyed(&path, &key, false)?;
        if is_new {
            schema::initialize(&mut connection)?;
            File::open(path.parent().ok_or(ArchiveError::InvalidStore)?)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| ArchiveError::StorageFailure)?;
        }
        schema::validate(&connection)?;
        validate_registrations(&connection, &validators)?;
        super::ingest_state::validate_runs(&connection, &validators)?;
        // Interrupted marking is the first mutation of an accepted original.
        // Staged preflight, schema and every registration/run must pass first.
        super::ingest_state::interrupt_runs(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            key,
            path,
            validators,
            import_limits: model::ImportLimits::default(),
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            before_maintenance: None,
            _process_lock: process_lock,
        })
    }

    pub(crate) fn register_source(
        &self,
        account: AccountRecord,
        source: SourceRecord,
    ) -> Result<SourceRecord, ArchiveError> {
        let validator = self
            .validators
            .get(&account.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        let native = model::validate_archive_account(&account, validator.as_ref())?;
        self.transaction(|tx| {
            let retired: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM cleanup_tasks WHERE source_id=?)", [source.id.as_uuid().to_string()], |row| row.get(0)).map_err(|_| ArchiveError::StorageFailure)?;
            if retired { return Err(ArchiveError::InvalidRecord); }
            let previous: Option<(String, String, String)> = tx.query_row(
                "SELECT provider, canonical_identity, record_json FROM accounts WHERE account_id = ?",
                [account.id.as_uuid().to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(|_| ArchiveError::StorageFailure)?;
            match previous {
                Some((provider, identity, _)) if provider != account.provider.as_str() || identity != native.as_canonical_str() => {
                    return Err(ArchiveError::InvalidRecord);
                }
                Some((_, _, encoded)) => {
                    let retained: AccountRecord = model::decode(&encoded)?;
                    model::validate_registration(&retained, &source, validator.as_ref())?;
                }
                None => {
                    model::validate_registration(&account, &source, validator.as_ref())?;
                    tx.execute("INSERT INTO accounts(provider, account_id, canonical_identity, record_json) VALUES(?, ?, ?, ?)",
                        (account.provider.as_str(), account.id.as_uuid().to_string(), native.as_canonical_str(), model::encode(&account)?))
                        .map_err(|_| ArchiveError::InvalidRecord)?;
                }
            }
            let previous: Option<String> = tx.query_row("SELECT record_json FROM sources WHERE source_id = ?",
                [source.id.as_uuid().to_string()], |row| row.get(0))
                .optional().map_err(|_| ArchiveError::StorageFailure)?;
            if let Some(encoded) = previous {
                let existing: SourceRecord = model::decode(&encoded)?;
                if existing.scope() != source.scope() || existing.kind != source.kind
                    || existing.archive_fingerprint != source.archive_fingerprint
                    || existing.schema_profile != source.schema_profile {
                    return Err(ArchiveError::InvalidRecord);
                }
                return Ok(existing);
            }
            if source.state != SourceState::Preparing || source.imported_at.is_some() {
                return Err(ArchiveError::InvalidRecord);
            }
            tx.execute("INSERT INTO sources(provider, account_id, source_id, record_json) VALUES(?, ?, ?, ?)",
                (source.provider.as_str(), source.account_id.as_uuid().to_string(), source.id.as_uuid().to_string(), model::encode(&source)?))
                .map_err(|_| ArchiveError::InvalidRecord)?;
            Ok(source)
        })
    }

    pub(crate) fn source(&self, scope: &Scope) -> Result<SourceRecord, ArchiveError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ArchiveError::StorageFailure)?;
        let encoded: Option<String> = connection.query_row(
            "SELECT record_json FROM sources WHERE provider = ? AND account_id = ? AND source_id = ?",
            (scope.provider.as_str(), scope.account_id.as_uuid().to_string(), scope.source_id.as_uuid().to_string()), |row| row.get(0),
        ).optional().map_err(|_| ArchiveError::StorageFailure)?;
        let source: SourceRecord = model::decode(&encoded.ok_or(ArchiveError::ScopeMismatch)?)?;
        if &source.scope() != scope {
            return Err(ArchiveError::ScopeMismatch);
        }
        Ok(source)
    }

    pub(super) fn transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, ArchiveError>,
    ) -> Result<T, ArchiveError> {
        self.transaction_checked(operation, || Ok(()))
    }

    pub(super) fn transaction_checked<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, ArchiveError>,
        before_commit: impl FnOnce() -> Result<(), ArchiveError>,
    ) -> Result<T, ArchiveError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ArchiveError::StorageFailure)?;
        validate_artifacts(&self.path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ArchiveError::StorageFailure)?;
        let result = operation(&transaction)?;
        #[cfg(test)]
        if let Some(hook) = &self.before_commit {
            hook();
        }
        before_commit()?;
        transaction
            .commit()
            .map_err(|_| ArchiveError::StorageFailure)?;
        Ok(result)
    }
}

fn validate_registrations(
    connection: &Connection,
    validators: &BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>,
) -> Result<(), ArchiveError> {
    let mut accounts = BTreeMap::new();
    let mut query = connection
        .prepare("SELECT provider, account_id, canonical_identity, record_json FROM accounts")
        .map_err(|_| ArchiveError::InvalidStore)?;
    let mut rows = query.query([]).map_err(|_| ArchiveError::InvalidStore)?;
    while let Some(row) = rows.next().map_err(|_| ArchiveError::InvalidStore)? {
        let (provider, id, native, encoded): (String, String, String, String) = (
            row.get(0).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(1).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(2).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(3).map_err(|_| ArchiveError::InvalidStore)?,
        );
        let account: AccountRecord = model::decode(&encoded)?;
        let validator = validators
            .get(&account.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        let verified = model::validate_archive_account(&account, validator.as_ref())?;
        if account.provider.as_str() != provider
            || account.id.as_uuid().to_string() != id
            || account.connection_state != ConnectionState::Disconnected
            || verified.as_canonical_str() != native
        {
            return Err(ArchiveError::InvalidRecord);
        }
        accounts.insert(account.id, account);
    }
    let mut query = connection
        .prepare("SELECT provider, account_id, source_id, record_json FROM sources")
        .map_err(|_| ArchiveError::InvalidStore)?;
    let mut rows = query.query([]).map_err(|_| ArchiveError::InvalidStore)?;
    while let Some(row) = rows.next().map_err(|_| ArchiveError::InvalidStore)? {
        let (provider, account_id, id, encoded): (String, String, String, String) = (
            row.get(0).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(1).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(2).map_err(|_| ArchiveError::InvalidStore)?,
            row.get(3).map_err(|_| ArchiveError::InvalidStore)?,
        );
        let source: SourceRecord = model::decode(&encoded)?;
        let account = accounts
            .get(&source.account_id)
            .ok_or(ArchiveError::InvalidRecord)?;
        let validator = validators
            .get(&source.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        model::validate_registration(account, &source, validator.as_ref())?;
        if source.provider.as_str() != provider
            || source.account_id.as_uuid().to_string() != account_id
            || source.id.as_uuid().to_string() != id
        {
            return Err(ArchiveError::InvalidRecord);
        }
    }
    Ok(())
}

pub(super) fn validate_parent(path: &Path) -> Result<(), ArchiveError> {
    let parent = path.parent().ok_or(ArchiveError::InvalidStore)?;
    if !path.is_absolute()
        || path.file_name().is_none()
        || fs::canonicalize(parent).map_err(|_| ArchiveError::InvalidStore)? != parent
    {
        return Err(ArchiveError::InvalidStore);
    }
    let metadata = fs::symlink_metadata(parent).map_err(|_| ArchiveError::InvalidStore)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(ArchiveError::InvalidStore);
    }
    Ok(())
}

pub(super) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub(super) fn validate_artifacts(path: &Path) -> Result<(), ArchiveError> {
    for suffix in ["", ".lock", "-wal", "-shm", "-journal"] {
        validate_file(&sidecar(path, suffix))?;
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), ArchiveError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.permissions().mode() & 0o077 != 0 =>
        {
            Err(ArchiveError::InvalidStore)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ArchiveError::StorageFailure),
    }
}

pub(super) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).mode(0o600);
    // O_NOFOLLOW values from the supported target ABIs. SQLite separately uses
    // SQLITE_OPEN_NOFOLLOW for the database handle; these protect the lock/open.
    #[cfg(target_os = "linux")]
    options.custom_flags(0x20000);
    #[cfg(target_os = "macos")]
    options.custom_flags(0x100);
    options
}

pub(super) fn acquire_lock(path: &Path) -> Result<ProcessLock, ArchiveError> {
    validate_file(path)?;
    let file = private_options()
        .create(true)
        .open(path)
        .map_err(|_| ArchiveError::InvalidStore)?;
    validate_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(ProcessLock(file)),
        Err(TryLockError::WouldBlock) => Err(ArchiveError::StoreInUse),
        Err(TryLockError::Error(_)) => Err(ArchiveError::StorageFailure),
    }
}
