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
    pub(crate) fn resolve_or_register_import(
        &self,
        input: model::NewArchiveImport,
    ) -> Result<model::ArchiveImportResolution, ArchiveError> {
        use super::ingest_state::{load_run, scope_sql, storage};
        use model::{ArchiveImportResolution, ImportDisposition, ImportPhase};
        input.bounded_size()?;
        let validator = self
            .validators
            .get(&input.provider)
            .ok_or(ArchiveError::InvalidRecord)?;
        let policy = validator.validation_policy_key();
        self.transaction(|tx| {
            // This non-persisted placeholder satisfies the neutral record shape
            // for provider validation. Allocate a real UUID only after lookup.
            let mut account = AccountRecord {
                id: uuid::Uuid::from_u128(1).try_into().map_err(|_| ArchiveError::InvalidRecord)?,
                provider: input.provider.clone(), native_identity: input.native_identity.clone(),
                display_name: input.display_name.clone(), username: input.username.clone(), avatar: input.avatar.clone(),
                connection_state: ConnectionState::Disconnected, created_at: input.observed_at, last_seen_at: input.observed_at,
            };
            let native = model::validate_archive_account(&account, validator.as_ref())?;
            let previous: Option<String> = tx.query_row("SELECT record_json FROM accounts WHERE provider=?1 AND canonical_identity=?2", (input.provider.as_str(), native.as_canonical_str()), |r| r.get(0)).optional().map_err(storage)?;
            if let Some(encoded) = previous {
                account = model::decode(&encoded)?;
                if account.provider != input.provider || model::validate_archive_account(&account, validator.as_ref())? != native { return Err(ArchiveError::InvalidStore); }
            } else {
                account.id = uuid::Uuid::new_v4().try_into().map_err(|_| ArchiveError::InvalidRecord)?;
                if model::validate_archive_account(&account, validator.as_ref())? != native { return Err(ArchiveError::InvalidRecord); }
                tx.execute("INSERT INTO accounts VALUES(?1, ?2, ?3, ?4)", (account.provider.as_str(), account.id.as_uuid().to_string(), native.as_canonical_str(), model::encode(&account)?)).map_err(storage)?;
            }
            let profile = model::encode(&input.schema_profile)?;
            let existing: Option<String> = tx.query_row("SELECT source_id FROM archive_import_identities WHERE provider=?1 AND account_id=?2 AND fingerprint=?3 AND schema_profile=?4 AND parser_policy=?5 AND validation_policy=?6", rusqlite::params![input.provider.as_str(), account.id.as_uuid().to_string(), input.fingerprint, profile, input.parser_policy, policy.as_str()], |r| r.get(0)).optional().map_err(storage)?;
            if let Some(id) = existing {
                let scope = Scope { provider: input.provider.clone(), account_id: account.id, source_id: super::ingest_state::uuid(&id)?.try_into().map_err(|_| ArchiveError::InvalidStore)? };
                let source = super::ingest_state::read_source(tx, &scope)?;
                model::validate_registration(&account, &source, validator.as_ref())?;
                let run = load_run(tx, &scope)?.ok_or(ArchiveError::InvalidStore)?;
                super::ingest_state::check_provenance(&source, &input.fingerprint, &input.schema_profile)?;
                super::ingest_state::validate_run(tx, &run, policy.as_str(), self.import_limits)?;
                let disposition = match run.checkpoint.progress.phase {
                    ImportPhase::Ready => ImportDisposition::Ready,
                    ImportPhase::Importing => ImportDisposition::Busy,
                    _ => ImportDisposition::RetryRequired,
                };
                return Ok(ArchiveImportResolution { account, source, checkpoint: run.checkpoint, disposition, session: None });
            }
            let source = SourceRecord {
                id: uuid::Uuid::new_v4().try_into().map_err(|_| ArchiveError::InvalidRecord)?,
                account_id: account.id, provider: input.provider, kind: retract_domain::SourceKind::ArchiveImport,
                state: SourceState::Preparing, archive_fingerprint: Some(input.fingerprint), schema_profile: input.schema_profile,
                imported_at: None, updated_at: input.observed_at, warnings: vec![],
            };
            model::validate_registration(&account, &source, validator.as_ref())?;
            let s = scope_sql(&source.scope());
            tx.execute("INSERT INTO sources VALUES(?1, ?2, ?3, ?4)", rusqlite::params![s[0], s[1], s[2], model::encode(&source)?]).map_err(storage)?;
            tx.execute("INSERT INTO archive_import_identities VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)", rusqlite::params![s[0], s[1], s[2], source.archive_fingerprint, profile, input.parser_policy, policy.as_str()]).map_err(storage)?;
            let session = super::ingest_state::create_run(tx, &source, policy.as_str())?;
            let checkpoint = load_run(tx, &source.scope())?.ok_or(ArchiveError::InvalidStore)?.checkpoint;
            Ok(ArchiveImportResolution { account, source, checkpoint, disposition: ImportDisposition::Start, session: Some(Arc::new(session)) })
        })
    }

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
            let mut old_schema = false;
            preflight::validate_existing(&path, &key, |connection| {
                match schema::validate(connection) {
                    Ok(()) => (),
                    Err(ArchiveError::UnsupportedSchema) => {
                        schema::validate_v1(connection)?;
                        old_schema = true;
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
                validate_registrations(connection, &validators)?;
                super::ingest_state::validate_runs(connection, &validators)
            })?;
            if old_schema {
                super::migration::upgrade_v1(&path, &key, &validators)?;
            }
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
        model::registration_bounds(&account, &source)?;
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

pub(super) fn validate_registrations(
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
