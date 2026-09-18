use std::{path::PathBuf, sync::Mutex};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::vault_lock::CredentialLease;
use super::{KEY_LENGTH, random_bytes};
use crate::error::AppError;

pub(super) const VAULT_ACCOUNT: &str = "secret-vault-v1";

pub(super) trait VaultIo {
    fn read(&mut self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, AppError>;
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError>;
}

pub(super) struct VaultCache {
    state: Mutex<CacheState>,
}

struct CacheState {
    vault: VaultState,
    root: Option<PathBuf>,
    require_lease: bool,
    lease: Option<CredentialLease>,
    closed: bool,
}

#[cfg(test)]
impl Default for VaultCache {
    fn default() -> Self {
        Self {
            state: Mutex::new(CacheState {
                vault: VaultState::Unloaded,
                root: None,
                require_lease: false,
                lease: None,
                closed: false,
            }),
        }
    }
}

#[derive(Default)]
enum VaultState {
    #[default]
    Unloaded,
    Ready(ReadyVault),
    Failed(String),
}

struct ReadyVault {
    vault: SecretVault,
    persisted: bool,
    needs_read: bool,
}

impl VaultCache {
    pub(super) fn application() -> Self {
        Self {
            state: Mutex::new(CacheState {
                vault: VaultState::Unloaded,
                root: None,
                require_lease: true,
                lease: None,
                closed: false,
            }),
        }
    }

    /// Binding is lexical only; the first credential operation validates files.
    pub(super) fn bind_root(&self, root: PathBuf) -> Result<(), AppError> {
        if !root.is_absolute()
            || root.file_name().is_none()
            || root.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(AppError::StateUnavailable);
        }
        let mut state = self.state.lock().map_err(|_| AppError::StateUnavailable)?;
        if state.closed {
            return Err(AppError::StateUnavailable);
        }
        if state
            .root
            .as_ref()
            .is_some_and(|existing| existing != &root)
        {
            return Err(AppError::StateUnavailable);
        }
        state.root = Some(root);
        Ok(())
    }

    pub(super) fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            // Closing is permanent. Drop zeroizing cached values before the
            // lease, with the mutex excluding late settings/credential access.
            state.closed = true;
            state.vault = VaultState::Unloaded;
            drop(state.lease.take());
        }
    }

    pub(super) fn api_hash(
        &self,
        io: &mut impl VaultIo,
    ) -> Result<Option<Zeroizing<String>>, AppError> {
        self.with_ready(io, |ready, _| {
            Ok(ready
                .vault
                .telegram_api_hash
                .as_ref()
                .map(|hash| Zeroizing::new(hash.clone())))
        })
    }

    pub(super) fn named_key(
        &self,
        io: &mut impl VaultIo,
        account: &str,
    ) -> Result<[u8; KEY_LENGTH], AppError> {
        self.with_ready(io, |ready, io| {
            if let Some(key) = named_key(&ready.vault, account)? {
                return Ok(key);
            }
            reconcile(ready, io)?;
            if let Some(key) = named_key(&ready.vault, account)? {
                return Ok(key);
            }
            let mut candidate = ready.vault.clone();
            let key = Zeroizing::new(random_bytes::<KEY_LENGTH>()?);
            match account {
                "tdlib-database" => candidate.tdlib_database_key = Some(*key),
                "encrypted-job-store" => candidate.job_store_key = Some(*key),
                _ => return Err(unsupported_account()),
            }
            commit(ready, io, candidate)?;
            Ok(*key)
        })
    }

    pub(super) fn save_api_hash(&self, io: &mut impl VaultIo, value: &str) -> Result<(), AppError> {
        if !valid_api_hash(value) {
            return Err(AppError::SecureStore(
                "Telegram API hash must be exactly 32 hexadecimal characters".into(),
            ));
        }
        self.with_ready(io, |ready, io| {
            reconcile(ready, io)?;
            let mut candidate = ready.vault.clone();
            candidate.telegram_api_hash.zeroize();
            candidate.telegram_api_hash = Some(value.to_owned());
            commit(ready, io, candidate)
        })
    }

    pub(super) fn discord_credential(
        &self,
        io: &mut impl VaultIo,
    ) -> Result<Option<(String, Zeroizing<String>)>, AppError> {
        self.with_ready(io, |ready, _| {
            Ok(ready.vault.discord_credential.as_ref().map(|credential| {
                (
                    credential.account_id.clone(),
                    Zeroizing::new(credential.token.clone()),
                )
            }))
        })
    }

    pub(super) fn save_discord_credential(
        &self,
        io: &mut impl VaultIo,
        account_id: &str,
        token: &str,
    ) -> Result<(), AppError> {
        if !valid_discord_account_id(account_id) || !valid_discord_token(token) {
            return Err(AppError::SecureStore(
                "Discord credential is malformed".into(),
            ));
        }
        self.with_ready(io, |ready, io| {
            reconcile(ready, io)?;
            let mut candidate = ready.vault.clone();
            candidate.discord_credential.zeroize();
            candidate.discord_credential = Some(DiscordVaultCredential {
                account_id: account_id.to_owned(),
                token: token.to_owned(),
            });
            candidate.version_three = true;
            commit(ready, io, candidate)
        })
    }

    pub(super) fn forget_discord_credential(&self, io: &mut impl VaultIo) -> Result<(), AppError> {
        self.with_ready(io, |ready, io| {
            reconcile(ready, io)?;
            if ready.vault.discord_credential.is_none() {
                return Ok(());
            }
            let mut candidate = ready.vault.clone();
            candidate.discord_credential.zeroize();
            candidate.discord_credential = None;
            candidate.version_three = true;
            commit(ready, io, candidate)
        })
    }

    // This key has no consumer until the archive store is integrated.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn archive_key(&self, io: &mut impl VaultIo) -> Result<[u8; KEY_LENGTH], AppError> {
        self.with_ready(io, |ready, io| {
            reconcile(ready, io)?;
            if let Some(key) = ready.vault.content_index_key {
                return Ok(key);
            }
            let mut candidate = ready.vault.clone();
            let key = Zeroizing::new(random_bytes::<KEY_LENGTH>()?);
            candidate.content_index_key = Some(*key);
            candidate.version_two = true;
            commit(ready, io, candidate)?;
            Ok(*key)
        })
    }

    fn with_ready<T, I: VaultIo>(
        &self,
        io: &mut I,
        operation: impl FnOnce(&mut ReadyVault, &mut I) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut state = self.state.lock().map_err(|_| AppError::StateUnavailable)?;
        if state.closed {
            return Err(AppError::StateUnavailable);
        }
        if state.require_lease && state.lease.is_none() {
            let root = state.root.as_ref().ok_or(AppError::StateUnavailable)?;
            state.lease = Some(CredentialLease::acquire(root)?);
        }
        if matches!(state.vault, VaultState::Unloaded) {
            state.vault = match load_vault(io) {
                Ok(ready) => VaultState::Ready(ready),
                Err(error) => VaultState::Failed(match error {
                    AppError::SecureStore(message) => message,
                    other => other.to_string(),
                }),
            };
        }
        match &mut state.vault {
            VaultState::Ready(ready) => operation(ready, io),
            VaultState::Failed(message) => Err(AppError::SecureStore(message.clone())),
            VaultState::Unloaded => unreachable!(),
        }
    }
}

fn unsupported_account() -> AppError {
    AppError::SecureStore("unsupported Retract secret account".into())
}

fn named_key(vault: &SecretVault, account: &str) -> Result<Option<[u8; KEY_LENGTH]>, AppError> {
    match account {
        "tdlib-database" => Ok(vault.tdlib_database_key),
        "encrypted-job-store" => Ok(vault.job_store_key),
        _ => Err(unsupported_account()),
    }
}

fn load_vault(io: &mut impl VaultIo) -> Result<ReadyVault, AppError> {
    if let Some(bytes) = io.read(VAULT_ACCOUNT)? {
        return Ok(ReadyVault {
            vault: decode_secret_vault(&bytes)?,
            persisted: true,
            needs_read: false,
        });
    }
    let mut vault = SecretVault::default();
    if let Some(bytes) = io.read("telegram-api-hash")? {
        let hash = std::str::from_utf8(&bytes).map_err(|_| invalid_vault())?;
        if !valid_api_hash(hash) {
            return Err(invalid_vault());
        }
        vault.telegram_api_hash = Some(hash.to_owned());
    }
    if let Some(bytes) = io.read("tdlib-database")? {
        vault.tdlib_database_key = Some(bytes.as_slice().try_into().map_err(|_| invalid_vault())?);
    }
    if let Some(bytes) = io.read("encrypted-job-store")? {
        vault.job_store_key = Some(bytes.as_slice().try_into().map_err(|_| invalid_vault())?);
    }
    let migrated = vault.telegram_api_hash.is_some()
        || vault.tdlib_database_key.is_some()
        || vault.job_store_key.is_some();
    let mut ready = ReadyVault {
        vault,
        persisted: false,
        needs_read: false,
    };
    if migrated {
        let candidate = ready.vault.clone();
        commit(&mut ready, io, candidate)?;
    }
    Ok(ready)
}

fn invalid_vault() -> AppError {
    AppError::SecureStore("macOS Keychain contains an invalid Retract secret vault".into())
}

fn reconcile(ready: &mut ReadyVault, io: &mut impl VaultIo) -> Result<(), AppError> {
    if !ready.needs_read {
        return Ok(());
    }
    let (vault, persisted) = match io.read(VAULT_ACCOUNT)? {
        Some(bytes) => (decode_secret_vault(&bytes)?, true),
        None if !ready.persisted => (SecretVault::default(), false),
        None => return Err(invalid_vault()),
    };
    // Never replace a key already handed to a database with missing/different bytes.
    if ready.vault.content_index_key.is_some()
        && ready.vault.content_index_key != vault.content_index_key
    {
        return Err(invalid_vault());
    }
    ready.persisted = persisted;
    ready.vault = vault;
    ready.needs_read = false;
    Ok(())
}

fn commit(
    ready: &mut ReadyVault,
    io: &mut impl VaultIo,
    candidate: SecretVault,
) -> Result<(), AppError> {
    let encoded = encode_secret_vault(&candidate)?;
    // Even a write that returns an error may have reached Keychain. Keep the
    // verified cache, but require a read before any later mutation/generation.
    ready.needs_read = true;
    io.write(&encoded)?;
    let readback = io.read(VAULT_ACCOUNT)?.ok_or_else(invalid_vault)?;
    if readback.as_slice() != encoded.as_slice() {
        return Err(invalid_vault());
    }
    decode_secret_vault(&readback)?;
    ready.vault = candidate;
    ready.persisted = true;
    ready.needs_read = false;
    Ok(())
}

pub(super) const VAULT_MAGIC: &[u8; 7] = b"RTRCTV1";
const VAULT_V2_MAGIC: &[u8; 7] = b"RTRCTV2";
const VAULT_V3_MAGIC: &[u8; 7] = b"RTRCTV3";
const VAULT_API_HASH: u8 = 1 << 0;
const VAULT_TDLIB_DATABASE_KEY: u8 = 1 << 1;
const VAULT_JOB_STORE_KEY: u8 = 1 << 2;
const VAULT_KNOWN_FLAGS: u8 = VAULT_API_HASH | VAULT_TDLIB_DATABASE_KEY | VAULT_JOB_STORE_KEY;

#[derive(Clone, Default, Zeroize, ZeroizeOnDrop)]
pub(super) struct SecretVault {
    pub(super) telegram_api_hash: Option<String>,
    pub(super) tdlib_database_key: Option<[u8; KEY_LENGTH]>,
    pub(super) job_store_key: Option<[u8; KEY_LENGTH]>,
    pub(super) content_index_key: Option<[u8; KEY_LENGTH]>,
    pub(super) discord_credential: Option<DiscordVaultCredential>,
    pub(super) version_two: bool,
    pub(super) version_three: bool,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub(super) struct DiscordVaultCredential {
    account_id: String,
    token: String,
}

pub(super) fn encode_secret_vault(vault: &SecretVault) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if vault
        .telegram_api_hash
        .as_deref()
        .is_some_and(|value| !valid_api_hash(value))
    {
        return Err(AppError::SecureStore(
            "Keychain vault contains a malformed Telegram API hash".into(),
        ));
    }
    if vault.version_three || vault.discord_credential.is_some() {
        return encode_namespaced_v3_vault(vault);
    }
    if vault.version_two || vault.content_index_key.is_some() {
        return encode_namespaced_vault(vault);
    }
    let mut flags = 0_u8;
    if vault.telegram_api_hash.is_some() {
        flags |= VAULT_API_HASH;
    }
    if vault.tdlib_database_key.is_some() {
        flags |= VAULT_TDLIB_DATABASE_KEY;
    }
    if vault.job_store_key.is_some() {
        flags |= VAULT_JOB_STORE_KEY;
    }

    let mut encoded = Zeroizing::new(Vec::with_capacity(VAULT_MAGIC.len() + 1 + 96));
    encoded.extend_from_slice(VAULT_MAGIC);
    encoded.push(flags);
    if let Some(value) = &vault.telegram_api_hash {
        encoded.extend_from_slice(value.as_bytes());
    }
    if let Some(value) = &vault.tdlib_database_key {
        encoded.extend_from_slice(value);
    }
    if let Some(value) = &vault.job_store_key {
        encoded.extend_from_slice(value);
    }
    Ok(encoded)
}

pub(super) fn decode_secret_vault(encoded: &[u8]) -> Result<SecretVault, AppError> {
    if encoded.starts_with(VAULT_V3_MAGIC) {
        return decode_namespaced_v3_vault(encoded);
    }
    if encoded.starts_with(VAULT_V2_MAGIC) {
        return decode_namespaced_vault(encoded);
    }
    if encoded.len() < VAULT_MAGIC.len() + 1 || !encoded.starts_with(VAULT_MAGIC) {
        return Err(AppError::SecureStore(
            "macOS Keychain contains an invalid Retract secret vault".into(),
        ));
    }
    let flags = encoded[VAULT_MAGIC.len()];
    if flags & !VAULT_KNOWN_FLAGS != 0 {
        return Err(AppError::SecureStore(
            "macOS Keychain contains an unsupported Retract secret vault".into(),
        ));
    }
    let mut cursor = VAULT_MAGIC.len() + 1;
    let mut vault = SecretVault::default();
    if flags & VAULT_API_HASH != 0 {
        let bytes = take_vault_field(encoded, &mut cursor)?;
        let value = std::str::from_utf8(bytes).map_err(|_| {
            AppError::SecureStore("macOS Keychain contains a malformed Telegram API hash".into())
        })?;
        if !valid_api_hash(value) {
            return Err(AppError::SecureStore(
                "macOS Keychain contains a malformed Telegram API hash".into(),
            ));
        }
        vault.telegram_api_hash = Some(value.to_owned());
    }
    if flags & VAULT_TDLIB_DATABASE_KEY != 0 {
        vault.tdlib_database_key = Some(
            take_vault_field(encoded, &mut cursor)?
                .try_into()
                .map_err(|_| {
                    AppError::SecureStore("malformed TDLib database key in Keychain vault".into())
                })?,
        );
    }
    if flags & VAULT_JOB_STORE_KEY != 0 {
        vault.job_store_key = Some(take_vault_field(encoded, &mut cursor)?.try_into().map_err(
            |_| AppError::SecureStore("malformed job-store key in Keychain vault".into()),
        )?);
    }
    if cursor != encoded.len() {
        return Err(AppError::SecureStore(
            "macOS Keychain contains a malformed Retract secret vault".into(),
        ));
    }
    Ok(vault)
}

fn take_vault_field<'a>(encoded: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], AppError> {
    let end = cursor.saturating_add(KEY_LENGTH);
    let field = encoded.get(*cursor..end).ok_or_else(|| {
        AppError::SecureStore("macOS Keychain contains a truncated Retract secret vault".into())
    })?;
    *cursor = end;
    Ok(field)
}

const VAULT_NAMES: [&str; 4] = [
    "telegram/legacy-profile/api-hash",
    "telegram/legacy-profile/tdlib-database-key",
    "retract/job-store-key",
    "retract/content-index-key",
];

// Header/count + four pairs of length bytes + 120 name bytes + four keys.
const MAX_V2_BYTES: usize = 264;

fn encode_namespaced_vault(vault: &SecretVault) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let values: [Option<&[u8]>; 4] = [
        vault.telegram_api_hash.as_deref().map(str::as_bytes),
        vault.tdlib_database_key.as_ref().map(|key| key.as_slice()),
        vault.job_store_key.as_ref().map(|key| key.as_slice()),
        vault.content_index_key.as_ref().map(|key| key.as_slice()),
    ];
    let mut encoded = Zeroizing::new(Vec::with_capacity(MAX_V2_BYTES));
    encoded.extend_from_slice(VAULT_V2_MAGIC);
    encoded.push(values.iter().flatten().count() as u8);
    for (name, value) in VAULT_NAMES.iter().zip(values) {
        if let Some(value) = value {
            encoded.push(name.len() as u8);
            encoded.extend_from_slice(name.as_bytes());
            encoded.push(KEY_LENGTH as u8);
            encoded.extend_from_slice(value);
        }
    }
    Ok(encoded)
}

fn decode_namespaced_vault(encoded: &[u8]) -> Result<SecretVault, AppError> {
    // v2: header, entry count, then name length/name/value length/32-byte value.
    // No extensions are silently discarded: an unknown entry blocks all writes.
    let invalid = invalid_vault;
    if encoded.len() < 8 || encoded.len() > MAX_V2_BYTES || encoded[7] > 4 {
        return Err(invalid());
    }
    let mut vault = SecretVault {
        version_two: true,
        telegram_api_hash: None,
        tdlib_database_key: None,
        job_store_key: None,
        content_index_key: None,
        discord_credential: None,
        version_three: false,
    };
    let mut seen = [false; 4];
    let mut cursor = 8;
    for _ in 0..encoded[7] {
        let name_len = usize::from(*encoded.get(cursor).ok_or_else(invalid)?);
        cursor += 1;
        let name = encoded.get(cursor..cursor + name_len).ok_or_else(invalid)?;
        cursor += name_len;
        let index = VAULT_NAMES
            .iter()
            .position(|expected| expected.as_bytes() == name)
            .ok_or_else(invalid)?;
        if seen[index] || encoded.get(cursor) != Some(&(KEY_LENGTH as u8)) {
            return Err(invalid());
        }
        seen[index] = true;
        cursor += 1;
        let value = take_vault_field(encoded, &mut cursor)?;
        match index {
            0 => {
                let hash = std::str::from_utf8(value).map_err(|_| invalid())?;
                if !valid_api_hash(hash) {
                    return Err(invalid());
                }
                vault.telegram_api_hash = Some(hash.to_owned());
            }
            1 => vault.tdlib_database_key = Some(value.try_into().map_err(|_| invalid())?),
            2 => vault.job_store_key = Some(value.try_into().map_err(|_| invalid())?),
            3 => vault.content_index_key = Some(value.try_into().map_err(|_| invalid())?),
            _ => unreachable!(),
        }
    }
    if cursor != encoded.len() {
        return Err(invalid());
    }
    Ok(vault)
}

pub(super) fn valid_api_hash(value: &str) -> bool {
    value.len() == KEY_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const DISCORD_VAULT_NAME: &str = "discord/session";
const MAX_DISCORD_TOKEN_BYTES: usize = 1024;
const MAX_V3_BYTES: usize = 1536;

fn encode_namespaced_v3_vault(vault: &SecretVault) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let mut entries: Vec<(&str, Zeroizing<Vec<u8>>)> = Vec::with_capacity(5);
    if let Some(value) = &vault.telegram_api_hash {
        entries.push((VAULT_NAMES[0], Zeroizing::new(value.as_bytes().to_vec())));
    }
    if let Some(value) = &vault.tdlib_database_key {
        entries.push((VAULT_NAMES[1], Zeroizing::new(value.to_vec())));
    }
    if let Some(value) = &vault.job_store_key {
        entries.push((VAULT_NAMES[2], Zeroizing::new(value.to_vec())));
    }
    if let Some(value) = &vault.content_index_key {
        entries.push((VAULT_NAMES[3], Zeroizing::new(value.to_vec())));
    }
    if let Some(credential) = &vault.discord_credential {
        if !valid_discord_account_id(&credential.account_id)
            || !valid_discord_token(&credential.token)
        {
            return Err(invalid_vault());
        }
        let account_len =
            u16::try_from(credential.account_id.len()).map_err(|_| invalid_vault())?;
        let mut value = Zeroizing::new(Vec::with_capacity(
            2 + credential.account_id.len() + credential.token.len(),
        ));
        value.extend_from_slice(&account_len.to_be_bytes());
        value.extend_from_slice(credential.account_id.as_bytes());
        value.extend_from_slice(credential.token.as_bytes());
        entries.push((DISCORD_VAULT_NAME, value));
    }

    let mut encoded = Zeroizing::new(Vec::with_capacity(MAX_V3_BYTES));
    encoded.extend_from_slice(VAULT_V3_MAGIC);
    encoded.push(u8::try_from(entries.len()).map_err(|_| invalid_vault())?);
    for (name, value) in entries {
        encoded.push(u8::try_from(name.len()).map_err(|_| invalid_vault())?);
        encoded.extend_from_slice(name.as_bytes());
        encoded.extend_from_slice(
            &u16::try_from(value.len())
                .map_err(|_| invalid_vault())?
                .to_be_bytes(),
        );
        encoded.extend_from_slice(&value);
    }
    if encoded.len() > MAX_V3_BYTES {
        return Err(invalid_vault());
    }
    Ok(encoded)
}

fn decode_namespaced_v3_vault(encoded: &[u8]) -> Result<SecretVault, AppError> {
    if encoded.len() < 8 || encoded.len() > MAX_V3_BYTES || encoded[7] > 5 {
        return Err(invalid_vault());
    }
    let all_names = [
        VAULT_NAMES[0],
        VAULT_NAMES[1],
        VAULT_NAMES[2],
        VAULT_NAMES[3],
        DISCORD_VAULT_NAME,
    ];
    let mut seen = [false; 5];
    let mut vault = SecretVault::default();
    vault.version_two = true;
    vault.version_three = true;
    let mut cursor = 8;
    for _ in 0..encoded[7] {
        let name_len = usize::from(*encoded.get(cursor).ok_or_else(invalid_vault)?);
        cursor += 1;
        let name_end = cursor.checked_add(name_len).ok_or_else(invalid_vault)?;
        let name = encoded.get(cursor..name_end).ok_or_else(invalid_vault)?;
        cursor = name_end;
        let index = all_names
            .iter()
            .position(|expected| expected.as_bytes() == name)
            .ok_or_else(invalid_vault)?;
        if seen[index] {
            return Err(invalid_vault());
        }
        seen[index] = true;
        let length_bytes: [u8; 2] = encoded
            .get(cursor..cursor + 2)
            .ok_or_else(invalid_vault)?
            .try_into()
            .map_err(|_| invalid_vault())?;
        cursor += 2;
        let value_len = usize::from(u16::from_be_bytes(length_bytes));
        let value_end = cursor.checked_add(value_len).ok_or_else(invalid_vault)?;
        let value = encoded.get(cursor..value_end).ok_or_else(invalid_vault)?;
        cursor = value_end;
        match index {
            0 => {
                let hash = std::str::from_utf8(value).map_err(|_| invalid_vault())?;
                if !valid_api_hash(hash) {
                    return Err(invalid_vault());
                }
                vault.telegram_api_hash = Some(hash.to_owned());
            }
            1 if value.len() == KEY_LENGTH => {
                vault.tdlib_database_key = Some(value.try_into().map_err(|_| invalid_vault())?);
            }
            2 if value.len() == KEY_LENGTH => {
                vault.job_store_key = Some(value.try_into().map_err(|_| invalid_vault())?);
            }
            3 if value.len() == KEY_LENGTH => {
                vault.content_index_key = Some(value.try_into().map_err(|_| invalid_vault())?);
            }
            4 => {
                let account_len_bytes: [u8; 2] = value
                    .get(..2)
                    .ok_or_else(invalid_vault)?
                    .try_into()
                    .map_err(|_| invalid_vault())?;
                let account_len = usize::from(u16::from_be_bytes(account_len_bytes));
                let account_end = 2_usize.checked_add(account_len).ok_or_else(invalid_vault)?;
                let account_id =
                    std::str::from_utf8(value.get(2..account_end).ok_or_else(invalid_vault)?)
                        .map_err(|_| invalid_vault())?;
                let token =
                    std::str::from_utf8(value.get(account_end..).ok_or_else(invalid_vault)?)
                        .map_err(|_| invalid_vault())?;
                if !valid_discord_account_id(account_id) || !valid_discord_token(token) {
                    return Err(invalid_vault());
                }
                vault.discord_credential = Some(DiscordVaultCredential {
                    account_id: account_id.to_owned(),
                    token: token.to_owned(),
                });
            }
            _ => return Err(invalid_vault()),
        }
    }
    if cursor != encoded.len() {
        return Err(invalid_vault());
    }
    Ok(vault)
}

fn valid_discord_account_id(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|parsed| parsed > 0) && !value.starts_with('0')
}

fn valid_discord_token(value: &str) -> bool {
    (20..=MAX_DISCORD_TOKEN_BYTES).contains(&value.len())
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_whitespace() && !byte.is_ascii_control())
        && !value.starts_with("Bot ")
        && !value.starts_with("Bearer ")
}
