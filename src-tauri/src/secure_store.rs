use std::fs;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
use std::sync::OnceLock;

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Generate, Payload},
};
use zeroize::Zeroizing;
#[cfg(test)]
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{error::AppError, model::PersistedState};

const MAGIC: &[u8; 7] = b"RTRCT02";
const LEGACY_UNBOUND_MAGIC: &[u8; 7] = b"RTRCT01";
pub(crate) const KEY_LENGTH: usize = 32;
pub(crate) const NONCE_LENGTH: usize = 12;
type AesNonce = aes_gcm::aead::Nonce<Aes256Gcm>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyCipherFormat {
    Rtrct01,
    Rtrct02,
}

#[cfg(any(target_os = "macos", test))]
mod vault;
#[cfg(test)]
use vault::{SecretVault, VAULT_MAGIC, decode_secret_vault, encode_secret_vault};
#[cfg(target_os = "macos")]
use vault::{VAULT_ACCOUNT, VaultCache, VaultIo};

#[cfg(target_os = "macos")]
static MAC_VAULT: OnceLock<VaultCache> = OnceLock::new();

#[cfg(test)]
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecureJobStore {
    key: [u8; KEY_LENGTH],
    #[zeroize(skip)]
    path: PathBuf,
    #[zeroize(skip)]
    profile_binding: Vec<u8>,
    #[zeroize(skip)]
    loaded_legacy_unbound: AtomicBool,
}

#[cfg(test)]
impl SecureJobStore {
    pub fn open(data_dir: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&data_dir)?;
        let key = load_job_store_key(&data_dir)?;
        let profile_binding = data_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned().into_bytes())
            .ok_or_else(|| AppError::SecureStore("job-store profile is invalid".into()))?;
        Ok(Self {
            key,
            path: data_dir.join("jobs.enc"),
            profile_binding,
            loaded_legacy_unbound: AtomicBool::new(false),
        })
    }

    pub fn open_setup(data_dir: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&data_dir)?;
        Ok(Self {
            key: [0x53; KEY_LENGTH],
            path: data_dir.join("setup-jobs.enc"),
            profile_binding: b"setup".to_vec(),
            loaded_legacy_unbound: AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    pub fn with_test_key(path: PathBuf, key: [u8; KEY_LENGTH]) -> Self {
        Self::with_test_key_and_profile(path, key, b"test")
    }

    #[cfg(test)]
    pub fn with_test_key_and_profile(
        path: PathBuf,
        key: [u8; KEY_LENGTH],
        profile_binding: &[u8],
    ) -> Self {
        Self {
            key,
            path,
            profile_binding: profile_binding.to_vec(),
            loaded_legacy_unbound: AtomicBool::new(false),
        }
    }

    pub fn loaded_legacy_unbound(&self) -> bool {
        self.loaded_legacy_unbound.load(Ordering::Acquire)
    }

    pub fn load(&self) -> Result<PersistedState, AppError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PersistedState::default());
            }
            Err(error) => return Err(error.into()),
        };
        let (state, format) =
            decode_legacy_state(&bytes, &self.key, self.profile_binding.as_slice())?;
        self.loaded_legacy_unbound
            .store(format == LegacyCipherFormat::Rtrct01, Ordering::Release);
        Ok(state)
    }

    pub fn save(&self, state: &PersistedState) -> Result<(), AppError> {
        let plaintext =
            serde_json::to_vec(state).map_err(|error| AppError::SecureStore(error.to_string()))?;
        let payload = encrypt_authenticated(
            MAGIC,
            &self.key,
            self.profile_binding.as_slice(),
            plaintext.as_ref(),
        )?;

        let temporary = self.path.with_extension("enc.tmp");
        write_private(&temporary, &payload)?;
        fs::rename(temporary, &self.path)?;
        self.loaded_legacy_unbound.store(false, Ordering::Release);
        Ok(())
    }
}

pub fn load_tdlib_database_key(data_dir: &std::path::Path) -> Result<[u8; KEY_LENGTH], AppError> {
    load_or_create_named_key(data_dir, "tdlib-database", "tdlib-database.key")
}

pub(crate) fn load_job_store_key(data_dir: &std::path::Path) -> Result<[u8; KEY_LENGTH], AppError> {
    load_or_create_named_key(data_dir, "encrypted-job-store", "job-store.key")
}

pub(crate) fn decode_legacy_state(
    bytes: &[u8],
    key: &[u8; KEY_LENGTH],
    profile_binding: &[u8],
) -> Result<(PersistedState, LegacyCipherFormat), AppError> {
    if bytes.len() < MAGIC.len() + NONCE_LENGTH {
        return Err(AppError::SecureStore(
            "job store has an invalid header".into(),
        ));
    }
    let (format, aad) = match &bytes[..MAGIC.len()] {
        value if value == LEGACY_UNBOUND_MAGIC => (LegacyCipherFormat::Rtrct01, &[][..]),
        value if value == MAGIC => (LegacyCipherFormat::Rtrct02, profile_binding),
        _ => {
            return Err(AppError::SecureStore(
                "job store has an invalid header".into(),
            ));
        }
    };
    let plaintext = decrypt_authenticated(bytes, &bytes[..MAGIC.len()], key, aad)?;
    let state = serde_json::from_slice(&plaintext)
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    Ok((state, format))
}

pub(crate) fn encrypt_authenticated(
    magic: &[u8],
    key: &[u8; KEY_LENGTH],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, AppError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
    let nonce_bytes = random_bytes::<NONCE_LENGTH>()?;
    let nonce = AesNonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| AppError::SecureStore("job store encryption failed".into()))?;
    let mut payload = Vec::with_capacity(magic.len() + NONCE_LENGTH + ciphertext.len());
    payload.extend_from_slice(magic);
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    Ok(payload)
}

pub(crate) fn decrypt_authenticated(
    bytes: &[u8],
    magic: &[u8],
    key: &[u8; KEY_LENGTH],
    aad: &[u8],
) -> Result<Vec<u8>, AppError> {
    if bytes.len() < magic.len() + NONCE_LENGTH || !bytes.starts_with(magic) {
        return Err(AppError::SecureStore(
            "job store has an invalid header".into(),
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| AppError::SecureStore("invalid encryption key".into()))?;
    let nonce = AesNonce::try_from(&bytes[magic.len()..magic.len() + NONCE_LENGTH])
        .map_err(|_| AppError::SecureStore("job store has an invalid nonce".into()))?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &bytes[magic.len() + NONCE_LENGTH..],
                aad,
            },
        )
        .map_err(|_| AppError::SecureStore("job store authentication failed".into()))
}

#[cfg(target_os = "macos")]
pub fn clear_cached_secrets() {
    if let Some(cache) = MAC_VAULT.get() {
        cache.clear();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn clear_cached_secrets() {}

#[cfg(target_os = "macos")]
pub fn load_telegram_api_hash(
    _data_dir: &std::path::Path,
) -> Result<Option<Zeroizing<String>>, AppError> {
    MAC_VAULT
        .get_or_init(VaultCache::default)
        .api_hash(&mut MacVaultIo)
}

#[cfg(target_os = "macos")]
pub fn save_telegram_api_hash(_data_dir: &std::path::Path, value: &str) -> Result<(), AppError> {
    MAC_VAULT
        .get_or_init(VaultCache::default)
        .save_api_hash(&mut MacVaultIo, value)
}

/// Loaded only when an archive operation first needs its independent key.
#[cfg(target_os = "macos")]
#[allow(dead_code)] // The archive service will consume this in a later task.
pub(crate) fn load_archive_index_key() -> Result<crate::persistence::archive::ArchiveKey, AppError>
{
    MAC_VAULT
        .get_or_init(VaultCache::default)
        .archive_key(&mut MacVaultIo)
        .map(crate::persistence::archive::ArchiveKey::new)
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
    let nonce = AesNonce::try_from(&bytes[..NONCE_LENGTH])
        .map_err(|_| AppError::SecureStore("encrypted Telegram API hash is malformed".into()))?;
    let plaintext = cipher
        .decrypt(&nonce, &bytes[NONCE_LENGTH..])
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
    let nonce_bytes = random_bytes::<NONCE_LENGTH>()?;
    let nonce = AesNonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, value.as_bytes())
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
    MAC_VAULT
        .get_or_init(VaultCache::default)
        .named_key(&mut MacVaultIo, account)
}

#[cfg(target_os = "macos")]
struct MacVaultIo;

#[cfg(target_os = "macos")]
impl VaultIo for MacVaultIo {
    fn read(&mut self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        use security_framework::passwords::get_generic_password;

        const SERVICE: &str = "app.retract.cleaner";
        const ITEM_NOT_FOUND: i32 = -25300;
        match get_generic_password(SERVICE, account) {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(error) => Err(AppError::SecureStore(format!("macOS Keychain: {error}"))),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        use security_framework::passwords::set_generic_password;

        const SERVICE: &str = "app.retract.cleaner";
        // Keep the original item identity: old builds reject RTRCTV2 instead of
        // interpreting a different item as an absent vault and minting keys.
        set_generic_password(SERVICE, VAULT_ACCOUNT, bytes)
            .map_err(|error| AppError::SecureStore(format!("macOS Keychain: {error}")))
    }
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
    let key = random_bytes::<KEY_LENGTH>()?;
    write_private(&path, &key)?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> Result<[u8; N], AppError> {
    <[u8; N]>::try_generate()
        .map_err(|error| AppError::SecureStore(format!("secure random generation failed: {error}")))
}

pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "secure_store/vault_tests.rs"]
mod vault_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PersistedState;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    // Frozen once with Node.js crypto over hand-authored JSON, independently of
    // this module's Rust serializers and encryption implementation.
    const FROZEN_LEGACY_KEY: [u8; KEY_LENGTH] = [0x61; KEY_LENGTH];
    const FROZEN_CURRENT_KEY: [u8; KEY_LENGTH] = [0x62; KEY_LENGTH];
    const FROZEN_PROFILE: &[u8] = b"telegram-compatibility";

    fn frozen_expected_state() -> serde_json::Value {
        serde_json::json!({
            "plans": [
                {
                    "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                    "operation": "selected_messages",
                    "target_chat_id": -2101,
                    "target_sender_id": null,
                    "target_sender_name": null,
                    "chat_title": null,
                    "items": [
                        {
                            "chat_id": -2101,
                            "message_id": 8101,
                            "expected_reach": "everyone"
                        },
                        {
                            "chat_id": -2101,
                            "message_id": 8102,
                            "expected_reach": "everyone"
                        }
                    ],
                    "summary": {
                        "selected": 2,
                        "deleteForEveryone": 2,
                        "selfOnly": 0,
                        "cannotDelete": 0
                    },
                    "confirmation_tier": "low",
                    "fingerprint": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                    "created_at": "2026-01-02T03:04:05Z"
                },
                {
                    "id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                    "operation": "clear_history",
                    "target_chat_id": -2102,
                    "target_sender_id": null,
                    "target_sender_name": null,
                    "chat_title": "Synthetic history",
                    "items": [],
                    "summary": {
                        "selected": 0,
                        "deleteForEveryone": 0,
                        "selfOnly": 0,
                        "cannotDelete": 0
                    },
                    "confirmation_tier": "high",
                    "fingerprint": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    "created_at": "2026-01-02T03:05:05Z"
                }
            ],
            "jobs": [
                {
                    "id": "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                    "planId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                    "operation": "selected_messages",
                    "targetChatIds": [-2101],
                    "status": "completed",
                    "total": 2,
                    "deleted": 2,
                    "skipped": 0,
                    "failed": 0,
                    "nextBatch": 1,
                    "retryAfterSeconds": null,
                    "errorCodes": [],
                    "createdAt": "2026-01-02T03:04:06Z",
                    "updatedAt": "2026-01-02T03:04:07Z"
                },
                {
                    "id": "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
                    "planId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                    "operation": "clear_history",
                    "targetChatIds": [-2102],
                    "status": "running",
                    "total": 0,
                    "deleted": 0,
                    "skipped": 0,
                    "failed": 0,
                    "nextBatch": 0,
                    "retryAfterSeconds": 1,
                    "errorCodes": ["telegram_rate_limited"],
                    "createdAt": "2026-01-02T03:05:06Z",
                    "updatedAt": "2026-01-02T03:05:07Z"
                }
            ]
        })
    }

    fn decode_frozen_fixture(raw: &str) -> Vec<u8> {
        BASE64
            .decode(raw.trim())
            .expect("valid frozen encrypted fixture base64")
    }

    fn decrypt_frozen_value(
        bytes: &[u8],
        key: [u8; KEY_LENGTH],
        profile: &[u8],
    ) -> serde_json::Value {
        assert!(bytes.len() >= MAGIC.len() + NONCE_LENGTH);
        let aad = match &bytes[..MAGIC.len()] {
            value if value == LEGACY_UNBOUND_MAGIC => &[][..],
            value if value == MAGIC => profile,
            value => panic!("unexpected frozen fixture magic: {value:?}"),
        };
        let cipher = Aes256Gcm::new_from_slice(&key).expect("valid frozen fixture key");
        let nonce = AesNonce::try_from(&bytes[MAGIC.len()..MAGIC.len() + NONCE_LENGTH])
            .expect("valid frozen fixture nonce");
        let plaintext = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &bytes[MAGIC.len() + NONCE_LENGTH..],
                    aad,
                },
            )
            .expect("independently authenticated frozen fixture plaintext");
        serde_json::from_slice(&plaintext).expect("frozen fixture plaintext JSON")
    }

    fn expect_raw_keys(
        value: &serde_json::Value,
        path: &str,
        expected: &[&str],
    ) -> Result<(), String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("{path} must be an object"))?;
        let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
        let mut expected = expected.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{path} raw keys differ: expected {expected:?}, found {actual:?}"
            ))
        }
    }

    fn expect_raw_array<'a>(
        value: &'a serde_json::Value,
        path: &str,
    ) -> Result<&'a [serde_json::Value], String> {
        value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{path} must be an array"))
    }

    fn validate_frozen_raw(value: &serde_json::Value) -> Result<(), String> {
        const FORBIDDEN_RAW_NAMES: &[&str] = &[
            "preview",
            "caption",
            "fileName",
            "file_name",
            "attachment",
            "messageBody",
            "message_body",
            "apiHash",
            "api_hash",
            "password",
            "authCode",
            "auth_code",
            "SYNTHETIC_CONTENT_SENTINEL",
            "SYNTHETIC_AUTH_SENTINEL",
        ];
        let raw = serde_json::to_string(value).map_err(|error| error.to_string())?;
        if let Some(name) = FORBIDDEN_RAW_NAMES.iter().find(|name| raw.contains(**name)) {
            return Err(format!("raw plaintext contains forbidden name {name}"));
        }

        expect_raw_keys(value, "$", &["plans", "jobs"])?;
        for (plan_index, plan) in expect_raw_array(&value["plans"], "$.plans")?
            .iter()
            .enumerate()
        {
            let plan_path = format!("$.plans[{plan_index}]");
            expect_raw_keys(
                plan,
                &plan_path,
                &[
                    "id",
                    "operation",
                    "target_chat_id",
                    "target_sender_id",
                    "target_sender_name",
                    "chat_title",
                    "items",
                    "summary",
                    "confirmation_tier",
                    "fingerprint",
                    "created_at",
                ],
            )?;
            for (item_index, item) in
                expect_raw_array(&plan["items"], &format!("{plan_path}.items"))?
                    .iter()
                    .enumerate()
            {
                expect_raw_keys(
                    item,
                    &format!("{plan_path}.items[{item_index}]"),
                    &["chat_id", "message_id", "expected_reach"],
                )?;
            }
            expect_raw_keys(
                &plan["summary"],
                &format!("{plan_path}.summary"),
                &["selected", "deleteForEveryone", "selfOnly", "cannotDelete"],
            )?;
        }
        for (job_index, job) in expect_raw_array(&value["jobs"], "$.jobs")?
            .iter()
            .enumerate()
        {
            expect_raw_keys(
                job,
                &format!("$.jobs[{job_index}]"),
                &[
                    "id",
                    "planId",
                    "operation",
                    "targetChatIds",
                    "status",
                    "total",
                    "deleted",
                    "skipped",
                    "failed",
                    "nextBatch",
                    "retryAfterSeconds",
                    "errorCodes",
                    "createdAt",
                    "updatedAt",
                ],
            )?;
            expect_raw_array(
                &job["targetChatIds"],
                &format!("$.jobs[{job_index}].targetChatIds"),
            )?;
            expect_raw_array(
                &job["errorCodes"],
                &format!("$.jobs[{job_index}].errorCodes"),
            )?;
        }
        Ok(())
    }

    fn assert_frozen_fixture(
        encoded: &str,
        key: [u8; KEY_LENGTH],
        profile: &[u8],
        legacy_unbound: bool,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let original = decode_frozen_fixture(encoded);
        let raw = decrypt_frozen_value(&original, key, profile);
        validate_frozen_raw(&raw)
            .expect("raw frozen fixture schema must be exact and content-free");
        assert_eq!(raw, frozen_expected_state());
        let independently_deserialized: PersistedState =
            serde_json::from_value(raw).expect("validated frozen fixture state");
        assert_eq!(
            serde_json::to_value(&independently_deserialized).unwrap(),
            frozen_expected_state()
        );

        fs::write(&path, &original).unwrap();
        let store = SecureJobStore::with_test_key_and_profile(path.clone(), key, profile);
        let state = store.load().unwrap();
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            frozen_expected_state()
        );
        assert_eq!(store.loaded_legacy_unbound(), legacy_unbound);

        let serialized = serde_json::to_string(&state).unwrap();
        for forbidden in [
            "preview",
            "caption",
            "fileName",
            "attachment",
            "apiHash",
            "password",
            "authCode",
        ] {
            assert!(!serialized.contains(forbidden), "found {forbidden}");
        }

        let mut tampered = original;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        fs::write(&path, tampered).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn rejects_unknown_keys_and_content_or_auth_names_in_raw_frozen_plaintext() {
        const FIXTURE: &str = include_str!("../tests/fixtures/secure-store/rtrct02-nonempty.b64");
        let original = decode_frozen_fixture(FIXTURE);
        let raw = decrypt_frozen_value(&original, FROZEN_CURRENT_KEY, FROZEN_PROFILE);

        let mut unknown_key_mutation = raw.clone();
        unknown_key_mutation["plans"][0]
            .as_object_mut()
            .unwrap()
            .insert(
                "unknown_field".into(),
                serde_json::Value::String("synthetic-value".into()),
            );
        let unknown_key_error = validate_frozen_raw(&unknown_key_mutation).unwrap_err();
        assert!(unknown_key_error.contains("unknown_field"));

        let mut content_mutation = raw.clone();
        content_mutation["jobs"][0]["errorCodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::Value::String(
                "SYNTHETIC_CONTENT_SENTINEL".into(),
            ));
        let content_error = validate_frozen_raw(&content_mutation).unwrap_err();
        assert!(content_error.contains("SYNTHETIC_CONTENT_SENTINEL"));

        let mut auth_mutation = raw;
        auth_mutation["jobs"][0]["errorCodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::Value::String("SYNTHETIC_AUTH_SENTINEL".into()));
        let auth_error = validate_frozen_raw(&auth_mutation).unwrap_err();
        assert!(auth_error.contains("SYNTHETIC_AUTH_SENTINEL"));
    }

    #[test]
    fn loads_frozen_nonempty_rtrct01_state_without_current_struct_serialization() {
        const FIXTURE: &str = include_str!("../tests/fixtures/secure-store/rtrct01-nonempty.b64");
        assert_frozen_fixture(FIXTURE, FROZEN_LEGACY_KEY, b"synthetic-other-profile", true);
    }

    #[test]
    fn loads_frozen_nonempty_rtrct02_state_with_profile_binding() {
        const FIXTURE: &str = include_str!("../tests/fixtures/secure-store/rtrct02-nonempty.b64");
        assert_frozen_fixture(FIXTURE, FROZEN_CURRENT_KEY, FROZEN_PROFILE, false);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, decode_frozen_fixture(FIXTURE)).unwrap();
        let wrong_profile = SecureJobStore::with_test_key_and_profile(
            path,
            FROZEN_CURRENT_KEY,
            b"synthetic-other-profile",
        );
        assert!(wrong_profile.load().is_err());
    }

    #[test]
    fn loads_job_store_written_by_aes_gcm_010() {
        const AES_GCM_010_FIXTURE: &[u8] = &[
            82, 84, 82, 67, 84, 48, 50, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 17, 239,
            11, 194, 18, 253, 131, 214, 66, 230, 179, 93, 246, 94, 231, 64, 115, 10, 226, 65, 34,
            72, 162, 142, 51, 220, 155, 170, 220, 251, 58, 97, 241, 142, 68, 129, 108, 100,
        ];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        fs::write(&path, AES_GCM_010_FIXTURE).unwrap();

        let store = SecureJobStore::with_test_key_and_profile(
            path,
            [0x2a; KEY_LENGTH],
            b"telegram-compatibility",
        );
        let state = store.load().unwrap();

        assert!(state.plans.is_empty());
        assert!(state.jobs.is_empty());
        assert!(!store.loaded_legacy_unbound());
    }

    #[test]
    fn loads_legacy_unbound_job_store_and_marks_it_for_safe_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [0x2a; KEY_LENGTH];
        let plaintext = serde_json::to_vec(&PersistedState::default()).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce_bytes = [0x11; NONCE_LENGTH];
        let nonce = AesNonce::from(nonce_bytes);
        let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref()).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(LEGACY_UNBOUND_MAGIC);
        bytes.extend_from_slice(&nonce_bytes);
        bytes.extend_from_slice(&ciphertext);
        fs::write(&path, bytes).unwrap();

        let store = SecureJobStore::with_test_key_and_profile(path, key, b"telegram-production");
        let state = store.load().unwrap();
        assert!(state.plans.is_empty());
        assert!(state.jobs.is_empty());
        assert!(store.loaded_legacy_unbound());
    }

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
    fn failed_temporary_write_preserves_the_last_authenticated_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let store = SecureJobStore::with_test_key(path.clone(), [8; KEY_LENGTH]);
        let expected = PersistedState::default();
        store.save(&expected).unwrap();
        let original = fs::read(&path).unwrap();

        fs::create_dir(path.with_extension("enc.tmp")).unwrap();
        assert!(store.save(&PersistedState::default()).is_err());

        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            serde_json::to_value(store.load().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn authenticated_job_state_is_bound_to_its_telegram_profile() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.enc");
        let key = [9; 32];
        SecureJobStore::with_test_key_and_profile(path.clone(), key, b"telegram-test")
            .save(&PersistedState::default())
            .unwrap();

        let production =
            SecureJobStore::with_test_key_and_profile(path, key, b"telegram-production");
        assert!(production.load().is_err());
    }

    #[test]
    fn keychain_vault_round_trips_all_startup_secrets() {
        let expected = SecretVault {
            telegram_api_hash: Some("0123456789abcdef0123456789abcdef".into()),
            tdlib_database_key: Some([0x2a; KEY_LENGTH]),
            job_store_key: Some([0x7c; KEY_LENGTH]),
            ..SecretVault::default()
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
            ..SecretVault::default()
        };
        let encoded = encode_secret_vault(&expected).unwrap();
        assert!(decode_secret_vault(&encoded[..encoded.len() - 1]).is_err());

        let mut unknown = encoded.to_vec();
        unknown[VAULT_MAGIC.len()] |= 1 << 7;
        assert!(decode_secret_vault(&unknown).is_err());
    }
}
