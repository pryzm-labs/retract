use std::{fs, path::PathBuf};

#[cfg(target_os = "macos")]
use std::sync::{Mutex, OnceLock};

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{error::AppError, model::PersistedState};

const MAGIC: &[u8; 7] = b"RTRCT01";
const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;

#[cfg(any(target_os = "macos", test))]
const VAULT_MAGIC: &[u8; 7] = b"RTRCTV1";
#[cfg(any(target_os = "macos", test))]
const VAULT_API_HASH: u8 = 1 << 0;
#[cfg(any(target_os = "macos", test))]
const VAULT_TDLIB_DATABASE_KEY: u8 = 1 << 1;
#[cfg(any(target_os = "macos", test))]
const VAULT_JOB_STORE_KEY: u8 = 1 << 2;
#[cfg(any(target_os = "macos", test))]
const VAULT_KNOWN_FLAGS: u8 = VAULT_API_HASH | VAULT_TDLIB_DATABASE_KEY | VAULT_JOB_STORE_KEY;

#[cfg(any(target_os = "macos", test))]
#[derive(Default, Zeroize, ZeroizeOnDrop)]
struct SecretVault {
    telegram_api_hash: Option<String>,
    tdlib_database_key: Option<[u8; KEY_LENGTH]>,
    job_store_key: Option<[u8; KEY_LENGTH]>,
}

#[cfg(target_os = "macos")]
enum MacVaultState {
    Unloaded,
    Ready(SecretVault),
    Failed(String),
}

#[cfg(target_os = "macos")]
static MAC_VAULT: OnceLock<Mutex<MacVaultState>> = OnceLock::new();

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecureJobStore {
    key: [u8; KEY_LENGTH],
    #[zeroize(skip)]
    path: PathBuf,
}

impl SecureJobStore {
    pub fn open(data_dir: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&data_dir)?;
        let key = load_or_create_named_key(&data_dir, "encrypted-job-store", "job-store.key")?;
        Ok(Self {
            key,
            path: data_dir.join("jobs.enc"),
        })
    }

    pub fn open_setup(data_dir: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&data_dir)?;
        Ok(Self {
            key: [0x53; KEY_LENGTH],
            path: data_dir.join("setup-jobs.enc"),
        })
    }

    #[cfg(test)]
    pub fn with_test_key(path: PathBuf, key: [u8; KEY_LENGTH]) -> Self {
        Self { key, path }
    }

    pub fn load(&self) -> Result<PersistedState, AppError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PersistedState::default());
            }
            Err(error) => return Err(error.into()),
        };
        if bytes.len() < MAGIC.len() + NONCE_LENGTH || &bytes[..MAGIC.len()] != MAGIC {
            return Err(AppError::SecureStore(
                "job store has an invalid header".into(),
            ));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
        let nonce = aes_gcm::Nonce::from_slice(&bytes[MAGIC.len()..MAGIC.len() + NONCE_LENGTH]);
        let plaintext = cipher
            .decrypt(nonce, &bytes[MAGIC.len() + NONCE_LENGTH..])
            .map_err(|_| AppError::SecureStore("job store authentication failed".into()))?;
        serde_json::from_slice(&plaintext).map_err(|error| AppError::SecureStore(error.to_string()))
    }

    pub fn save(&self, state: &PersistedState) -> Result<(), AppError> {
        let plaintext =
            serde_json::to_vec(state).map_err(|error| AppError::SecureStore(error.to_string()))?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
        let mut nonce_bytes = [0_u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|_| AppError::SecureStore("job store encryption failed".into()))?;

        let mut payload = Vec::with_capacity(MAGIC.len() + NONCE_LENGTH + ciphertext.len());
        payload.extend_from_slice(MAGIC);
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        let temporary = self.path.with_extension("enc.tmp");
        write_private(&temporary, &payload)?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

pub fn load_tdlib_database_key(data_dir: &std::path::Path) -> Result<[u8; KEY_LENGTH], AppError> {
    load_or_create_named_key(data_dir, "tdlib-database", "tdlib-database.key")
}

#[cfg(target_os = "macos")]
pub fn clear_cached_secrets() {
    if let Some(state) = MAC_VAULT.get()
        && let Ok(mut state) = state.lock()
    {
        *state = MacVaultState::Unloaded;
    }
}

#[cfg(not(target_os = "macos"))]
pub fn clear_cached_secrets() {}

#[cfg(target_os = "macos")]
pub fn load_telegram_api_hash(
    _data_dir: &std::path::Path,
) -> Result<Option<Zeroizing<String>>, AppError> {
    with_mac_secret_vault(|vault| {
        Ok(vault
            .telegram_api_hash
            .as_ref()
            .map(|value| Zeroizing::new(value.clone())))
    })
}

#[cfg(target_os = "macos")]
pub fn save_telegram_api_hash(_data_dir: &std::path::Path, value: &str) -> Result<(), AppError> {
    if !valid_api_hash(value) {
        return Err(AppError::SecureStore(
            "Telegram API hash must be exactly 32 hexadecimal characters".into(),
        ));
    }
    with_mac_secret_vault(|vault| {
        let previous = vault.telegram_api_hash.replace(value.to_owned());
        if let Err(error) = store_mac_secret_vault(vault) {
            vault.telegram_api_hash = previous;
            return Err(error);
        }
        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn load_telegram_api_hash(
    data_dir: &std::path::Path,
) -> Result<Option<Zeroizing<String>>, AppError> {
    let path = data_dir.join("telegram-api-hash.enc");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if bytes.len() < NONCE_LENGTH {
        return Err(AppError::SecureStore(
            "encrypted Telegram API hash is malformed".into(),
        ));
    }
    let key = load_or_create_named_key(data_dir, "connection-settings", "connection-settings.key")?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
    let plaintext = cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(&bytes[..NONCE_LENGTH]),
            &bytes[NONCE_LENGTH..],
        )
        .map_err(|_| AppError::SecureStore("Telegram API hash authentication failed".into()))?;
    String::from_utf8(plaintext)
        .map(Zeroizing::new)
        .map(Some)
        .map_err(|_| AppError::SecureStore("encrypted Telegram API hash is malformed".into()))
}

#[cfg(not(target_os = "macos"))]
pub fn save_telegram_api_hash(data_dir: &std::path::Path, value: &str) -> Result<(), AppError> {
    let key = load_or_create_named_key(data_dir, "connection-settings", "connection-settings.key")?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
    let mut nonce_bytes = [0_u8; NONCE_LENGTH];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce_bytes), value.as_bytes())
        .map_err(|_| AppError::SecureStore("Telegram API hash encryption failed".into()))?;
    let mut payload = Vec::with_capacity(NONCE_LENGTH + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    fs::create_dir_all(data_dir)?;
    write_private(&data_dir.join("telegram-api-hash.enc"), &payload)
}

#[cfg(target_os = "macos")]
fn load_or_create_named_key(
    _data_dir: &std::path::Path,
    account: &str,
    _file_name: &str,
) -> Result<[u8; KEY_LENGTH], AppError> {
    with_mac_secret_vault(|vault| {
        if let Some(key) = vault_named_key(vault, account)? {
            return Ok(key);
        }

        let mut key = [0_u8; KEY_LENGTH];
        OsRng.fill_bytes(&mut key);
        set_vault_named_key(vault, account, Some(key))?;
        if let Err(error) = store_mac_secret_vault(vault) {
            set_vault_named_key(vault, account, None)?;
            key.zeroize();
            return Err(error);
        }
        Ok(key)
    })
}

#[cfg(target_os = "macos")]
fn with_mac_secret_vault<T>(
    operation: impl FnOnce(&mut SecretVault) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let state = MAC_VAULT.get_or_init(|| Mutex::new(MacVaultState::Unloaded));
    let mut state = state.lock().map_err(|_| AppError::StateUnavailable)?;

    if matches!(*state, MacVaultState::Unloaded) {
        *state = match load_mac_secret_vault() {
            Ok(vault) => MacVaultState::Ready(vault),
            Err(error) => MacVaultState::Failed(secure_store_message(error)),
        };
    }

    let result = match &mut *state {
        MacVaultState::Ready(vault) => operation(vault),
        MacVaultState::Failed(message) => {
            return Err(AppError::SecureStore(message.clone()));
        }
        MacVaultState::Unloaded => unreachable!("the Keychain vault must be initialized"),
    };

    if let Err(AppError::SecureStore(message)) = &result
        && message.starts_with("macOS Keychain:")
    {
        *state = MacVaultState::Failed(message.clone());
    }
    result
}

#[cfg(target_os = "macos")]
fn secure_store_message(error: AppError) -> String {
    match error {
        AppError::SecureStore(message) => message,
        other => other.to_string(),
    }
}

#[cfg(target_os = "macos")]
fn load_mac_secret_vault() -> Result<SecretVault, AppError> {
    use security_framework::passwords::get_generic_password;

    const SERVICE: &str = "app.retract.cleaner";
    const VAULT_ACCOUNT: &str = "secret-vault-v1";
    const ITEM_NOT_FOUND: i32 = -25300;

    match get_generic_password(SERVICE, VAULT_ACCOUNT) {
        Ok(value) => decode_secret_vault(&value),
        Err(error) if error.code() == ITEM_NOT_FOUND => {
            let mut vault = SecretVault::default();
            let mut migrated = false;

            if let Some(value) = read_legacy_keychain_item("telegram-api-hash")? {
                let value = String::from_utf8(value).map_err(|_| {
                    AppError::SecureStore(
                        "macOS Keychain contains a malformed Telegram API hash".into(),
                    )
                })?;
                if !valid_api_hash(&value) {
                    return Err(AppError::SecureStore(
                        "macOS Keychain contains a malformed Telegram API hash".into(),
                    ));
                }
                vault.telegram_api_hash = Some(value);
                migrated = true;
            }
            if let Some(value) = read_legacy_keychain_item("tdlib-database")? {
                vault.tdlib_database_key = Some(value.try_into().map_err(|_| {
                    AppError::SecureStore(
                        "macOS Keychain contains a malformed TDLib database key".into(),
                    )
                })?);
                migrated = true;
            }
            if let Some(value) = read_legacy_keychain_item("encrypted-job-store")? {
                vault.job_store_key = Some(value.try_into().map_err(|_| {
                    AppError::SecureStore(
                        "macOS Keychain contains a malformed job-store key".into(),
                    )
                })?);
                migrated = true;
            }
            if migrated {
                store_mac_secret_vault(&vault)?;
            }
            Ok(vault)
        }
        Err(error) => Err(AppError::SecureStore(format!("macOS Keychain: {error}"))),
    }
}

#[cfg(target_os = "macos")]
fn read_legacy_keychain_item(account: &str) -> Result<Option<Vec<u8>>, AppError> {
    use security_framework::passwords::get_generic_password;

    const SERVICE: &str = "app.retract.cleaner";
    const ITEM_NOT_FOUND: i32 = -25300;
    match get_generic_password(SERVICE, account) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(AppError::SecureStore(format!("macOS Keychain: {error}"))),
    }
}

#[cfg(target_os = "macos")]
fn store_mac_secret_vault(vault: &SecretVault) -> Result<(), AppError> {
    use security_framework::passwords::set_generic_password;

    const SERVICE: &str = "app.retract.cleaner";
    const VAULT_ACCOUNT: &str = "secret-vault-v1";
    let encoded = encode_secret_vault(vault)?;
    set_generic_password(SERVICE, VAULT_ACCOUNT, encoded.as_slice())
        .map_err(|error| AppError::SecureStore(format!("macOS Keychain: {error}")))
}

#[cfg(target_os = "macos")]
fn vault_named_key(
    vault: &SecretVault,
    account: &str,
) -> Result<Option<[u8; KEY_LENGTH]>, AppError> {
    match account {
        "tdlib-database" => Ok(vault.tdlib_database_key),
        "encrypted-job-store" => Ok(vault.job_store_key),
        _ => Err(AppError::SecureStore(format!(
            "unsupported Retract secret account: {account}"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn set_vault_named_key(
    vault: &mut SecretVault,
    account: &str,
    value: Option<[u8; KEY_LENGTH]>,
) -> Result<(), AppError> {
    match account {
        "tdlib-database" => vault.tdlib_database_key = value,
        "encrypted-job-store" => vault.job_store_key = value,
        _ => {
            return Err(AppError::SecureStore(format!(
                "unsupported Retract secret account: {account}"
            )));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn encode_secret_vault(vault: &SecretVault) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if vault
        .telegram_api_hash
        .as_deref()
        .is_some_and(|value| !valid_api_hash(value))
    {
        return Err(AppError::SecureStore(
            "Keychain vault contains a malformed Telegram API hash".into(),
        ));
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

#[cfg(any(target_os = "macos", test))]
fn decode_secret_vault(encoded: &[u8]) -> Result<SecretVault, AppError> {
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
        let value = String::from_utf8(bytes.to_vec()).map_err(|_| {
            AppError::SecureStore("macOS Keychain contains a malformed Telegram API hash".into())
        })?;
        if !valid_api_hash(&value) {
            return Err(AppError::SecureStore(
                "macOS Keychain contains a malformed Telegram API hash".into(),
            ));
        }
        vault.telegram_api_hash = Some(value);
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

#[cfg(any(target_os = "macos", test))]
fn take_vault_field<'a>(encoded: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], AppError> {
    let end = cursor.saturating_add(KEY_LENGTH);
    let field = encoded.get(*cursor..end).ok_or_else(|| {
        AppError::SecureStore("macOS Keychain contains a truncated Retract secret vault".into())
    })?;
    *cursor = end;
    Ok(field)
}

#[cfg(any(target_os = "macos", test))]
fn valid_api_hash(value: &str) -> bool {
    value.len() == KEY_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_named_key(
    data_dir: &std::path::Path,
    _account: &str,
    file_name: &str,
) -> Result<[u8; KEY_LENGTH], AppError> {
    use std::io::Read;

    let path = data_dir.join(file_name);
    if path.exists() {
        let mut key = [0_u8; KEY_LENGTH];
        fs::File::open(path)?.read_exact(&mut key)?;
        return Ok(key);
    }
    let mut key = [0_u8; KEY_LENGTH];
    OsRng.fill_bytes(&mut key);
    write_private(&path, &key)?;
    Ok(key)
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PersistedState;

    #[test]
    fn round_trip_and_tamper_detection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let store = SecureJobStore::with_test_key(path.clone(), [7; 32]);
        store.save(&PersistedState::default()).unwrap();
        assert_eq!(store.load().unwrap().jobs.len(), 0);

        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn keychain_vault_round_trips_all_startup_secrets() {
        let expected = SecretVault {
            telegram_api_hash: Some("0123456789abcdef0123456789abcdef".into()),
            tdlib_database_key: Some([0x2a; KEY_LENGTH]),
            job_store_key: Some([0x7c; KEY_LENGTH]),
        };
        let encoded = encode_secret_vault(&expected).unwrap();
        let decoded = decode_secret_vault(&encoded).unwrap();

        assert_eq!(decoded.telegram_api_hash, expected.telegram_api_hash);
        assert_eq!(decoded.tdlib_database_key, expected.tdlib_database_key);
        assert_eq!(decoded.job_store_key, expected.job_store_key);
    }

    #[test]
    fn keychain_vault_rejects_truncated_or_unknown_records() {
        let expected = SecretVault {
            telegram_api_hash: Some("0123456789abcdef0123456789abcdef".into()),
            tdlib_database_key: Some([0x2a; KEY_LENGTH]),
            job_store_key: None,
        };
        let encoded = encode_secret_vault(&expected).unwrap();
        assert!(decode_secret_vault(&encoded[..encoded.len() - 1]).is_err());

        let mut unknown = encoded.to_vec();
        unknown[VAULT_MAGIC.len()] |= 1 << 7;
        assert!(decode_secret_vault(&unknown).is_err());
    }
}
