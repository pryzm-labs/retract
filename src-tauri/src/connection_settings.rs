use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

use crate::{error::AppError, live_gateway::SUPPORTED_TDLIB_VERSION, secure_store};

const SETTINGS_FILE: &str = "connection-settings.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePreference {
    Demo,
    #[default]
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnectionSettings {
    #[serde(default = "settings_schema_version")]
    schema_version: u8,
    #[serde(default)]
    setup_complete: bool,
    #[serde(default)]
    runtime_mode: RuntimePreference,
    #[serde(default)]
    tdlib_path: Option<PathBuf>,
    #[serde(default)]
    api_id: Option<i32>,
    #[serde(default)]
    use_test_dc: bool,
}

impl Default for StoredConnectionSettings {
    fn default() -> Self {
        Self {
            schema_version: settings_schema_version(),
            setup_complete: false,
            runtime_mode: RuntimePreference::Live,
            tdlib_path: None,
            api_id: None,
            use_test_dc: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSettingsView {
    pub setup_complete: bool,
    pub tdlib_path: String,
    pub detected_tdlib_path: Option<String>,
    pub bundled_tdlib_available: bool,
    pub api_id: Option<i32>,
    pub api_hash_configured: bool,
    pub use_test_dc: bool,
    pub environment_overrides: Vec<String>,
    pub configuration_error: Option<String>,
    pub supported_tdlib_version: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectionSettingsRequest {
    #[serde(default)]
    pub tdlib_path: String,
    pub api_id: Option<i32>,
    #[serde(default)]
    pub api_hash: Option<String>,
    #[serde(default)]
    pub use_test_dc: bool,
}

pub struct EffectiveLiveSettings {
    pub library_path: PathBuf,
    pub api_id: i32,
    pub api_hash: Zeroizing<String>,
    pub use_test_dc: bool,
}

pub fn get_view(app: &AppHandle) -> Result<ConnectionSettingsView, AppError> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    let (stored, configuration_error) = match load(&data_dir) {
        Ok(stored) => (stored, None),
        Err(error) => (StoredConnectionSettings::default(), Some(error.to_string())),
    };
    let bundled = bundled_tdlib_path(app);
    let detected = bundled.clone().or_else(detect_unbundled_tdlib_path);
    let environment_overrides = environment_overrides();
    let env_requests_live = environment_requests_live();

    let tdlib_path = std::env::var_os("RETRACT_TDLIB_PATH")
        .map(PathBuf::from)
        .or_else(|| bundled.clone())
        .or_else(|| stored.tdlib_path.clone())
        .or_else(|| detected.clone())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let api_id = std::env::var("RETRACT_TELEGRAM_API_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .or(stored.api_id);
    let should_check_keychain = env_requests_live || public_setup_complete(&stored);
    let api_hash_configured = std::env::var("RETRACT_TELEGRAM_API_HASH")
        .is_ok_and(|value| !value.trim().is_empty())
        || (should_check_keychain && secure_store::load_telegram_api_hash(&data_dir)?.is_some());
    let use_test_dc = std::env::var("RETRACT_TELEGRAM_TEST_DC")
        .map(|value| value == "1")
        .unwrap_or(stored.use_test_dc);

    Ok(ConnectionSettingsView {
        setup_complete: public_setup_complete(&stored) || env_requests_live,
        tdlib_path,
        detected_tdlib_path: detected.map(|path| path.to_string_lossy().into_owned()),
        bundled_tdlib_available: bundled.is_some(),
        api_id,
        api_hash_configured,
        use_test_dc,
        environment_overrides,
        configuration_error,
        supported_tdlib_version: SUPPORTED_TDLIB_VERSION,
    })
}

pub fn save(app: &AppHandle, request: SaveConnectionSettingsRequest) -> Result<(), AppError> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    let existing = load(&data_dir).unwrap_or_default();
    let supplied_hash = request
        .api_hash
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let tdlib_path = request.tdlib_path.trim();
    let detected_tdlib = detect_tdlib_path(app);
    let resolved_tdlib = if tdlib_path.is_empty() {
        detected_tdlib.clone()
    } else {
        Some(PathBuf::from(tdlib_path))
    };
    validate_live_fields(
        &resolved_tdlib
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        request.api_id,
        supplied_hash.is_some() || secure_store::load_telegram_api_hash(&data_dir)?.is_some(),
    )?;

    if let Some(api_hash) = supplied_hash {
        validate_api_hash(api_hash)?;
        secure_store::save_telegram_api_hash(&data_dir, api_hash)?;
    }

    let settings = StoredConnectionSettings {
        schema_version: settings_schema_version(),
        setup_complete: true,
        runtime_mode: RuntimePreference::Live,
        tdlib_path: resolved_tdlib.filter(|path| detected_tdlib.as_ref() != Some(path)),
        api_id: request.api_id.or(existing.api_id),
        use_test_dc: request.use_test_dc,
    };
    write_settings(&data_dir, &settings)
}

pub fn effective_live(app: &AppHandle) -> Result<Option<EffectiveLiveSettings>, AppError> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    let stored = load(&data_dir)?;
    let env_requests_live = environment_requests_live();
    if !env_requests_live && !public_setup_complete(&stored) {
        return Ok(None);
    }

    let library_path = std::env::var_os("RETRACT_TDLIB_PATH")
        .map(PathBuf::from)
        .or_else(|| bundled_tdlib_path(app))
        .or(stored.tdlib_path)
        .or_else(|| detect_tdlib_path(app))
        .ok_or_else(|| {
            AppError::InvalidRequest("choose the TDLib dynamic library in Retract Settings".into())
        })?;
    let api_id = match std::env::var("RETRACT_TELEGRAM_API_ID") {
        Ok(value) => value.parse::<i32>().map_err(|_| {
            AppError::InvalidRequest("RETRACT_TELEGRAM_API_ID must be an integer".into())
        })?,
        Err(_) => stored.api_id.ok_or_else(|| {
            AppError::InvalidRequest("enter a Telegram API ID in Retract Settings".into())
        })?,
    };
    let api_hash = match std::env::var("RETRACT_TELEGRAM_API_HASH") {
        Ok(value) if !value.trim().is_empty() => Zeroizing::new(value),
        _ => secure_store::load_telegram_api_hash(&data_dir)?.ok_or_else(|| {
            AppError::InvalidRequest("enter a Telegram API hash in Retract Settings".into())
        })?,
    };
    let use_test_dc = std::env::var("RETRACT_TELEGRAM_TEST_DC")
        .map(|value| value == "1")
        .unwrap_or(stored.use_test_dc);

    validate_live_fields(
        &library_path.to_string_lossy(),
        Some(api_id),
        !api_hash.trim().is_empty(),
    )?;
    validate_api_hash(api_hash.trim())?;
    Ok(Some(EffectiveLiveSettings {
        library_path,
        api_id,
        api_hash,
        use_test_dc,
    }))
}

fn load(data_dir: &Path) -> Result<StoredConnectionSettings, AppError> {
    let path = data_dir.join(SETTINGS_FILE);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            AppError::SecureStore(format!("invalid connection settings: {error}"))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(StoredConnectionSettings::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn write_settings(data_dir: &Path, settings: &StoredConnectionSettings) -> Result<(), AppError> {
    use std::io::Write;

    fs::create_dir_all(data_dir)?;
    let payload = serde_json::to_vec_pretty(settings)
        .map_err(|error| AppError::SecureStore(error.to_string()))?;
    let temporary = data_dir.join(format!("{SETTINGS_FILE}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    fs::rename(temporary, data_dir.join(SETTINGS_FILE))?;
    Ok(())
}

fn validate_live_fields(
    path: &str,
    api_id: Option<i32>,
    has_api_hash: bool,
) -> Result<(), AppError> {
    let library_path = Path::new(path);
    if path.is_empty() {
        return Err(AppError::InvalidRequest(
            "choose the TDLib dynamic library".into(),
        ));
    }
    if !library_path.is_absolute() {
        return Err(AppError::InvalidRequest(
            "the TDLib path must be absolute".into(),
        ));
    }
    if !library_path.is_file() {
        return Err(AppError::InvalidRequest(format!(
            "TDLib was not found at {}",
            library_path.display()
        )));
    }
    if api_id.is_none_or(|value| value <= 0) {
        return Err(AppError::InvalidRequest(
            "the Telegram API ID must be a positive number".into(),
        ));
    }
    if !has_api_hash {
        return Err(AppError::InvalidRequest(
            "enter the Telegram API hash".into(),
        ));
    }
    Ok(())
}

fn validate_api_hash(value: &str) -> Result<(), AppError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::InvalidRequest(
            "the Telegram API hash must be the 32-character hexadecimal value from my.telegram.org"
                .into(),
        ));
    }
    Ok(())
}

fn public_setup_complete(settings: &StoredConnectionSettings) -> bool {
    settings.setup_complete && settings.runtime_mode == RuntimePreference::Live
}

fn environment_overrides() -> Vec<String> {
    [
        "RETRACT_TDLIB_PATH",
        "RETRACT_TELEGRAM_API_ID",
        "RETRACT_TELEGRAM_API_HASH",
        "RETRACT_TELEGRAM_TEST_DC",
    ]
    .into_iter()
    .filter(|name| std::env::var_os(name).is_some())
    .map(str::to_owned)
    .collect()
}

fn environment_requests_live() -> bool {
    [
        "RETRACT_TDLIB_PATH",
        "RETRACT_TELEGRAM_API_ID",
        "RETRACT_TELEGRAM_API_HASH",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some())
}

fn detect_tdlib_path(app: &AppHandle) -> Option<PathBuf> {
    bundled_tdlib_path(app).or_else(detect_unbundled_tdlib_path)
}

fn bundled_tdlib_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("libtdjson.dylib"));
        candidates.push(resource_dir.join("lib/libtdjson.dylib"));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("vendor/tdlib-dist/libtdjson.dylib"));
        if let Some(parent) = current_dir.parent() {
            candidates.push(parent.join("vendor/tdlib-dist/libtdjson.dylib"));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn detect_unbundled_tdlib_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("vendor/tdlib-source/build/libtdjson.dylib"));
        candidates.push(current_dir.join("vendor/tdlib-source/build-retract/libtdjson.dylib"));
        if let Some(parent) = current_dir.parent() {
            candidates.push(parent.join("vendor/tdlib-source/build/libtdjson.dylib"));
            candidates.push(parent.join("vendor/tdlib-source/build-retract/libtdjson.dylib"));
        }
    }
    candidates.push(PathBuf::from("/opt/homebrew/lib/libtdjson.dylib"));
    candidates.push(PathBuf::from("/usr/local/lib/libtdjson.dylib"));
    candidates.into_iter().find(|path| path.is_file())
}

const fn settings_schema_version() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_never_serializes_the_api_hash() {
        let settings = StoredConnectionSettings {
            setup_complete: true,
            runtime_mode: RuntimePreference::Live,
            api_id: Some(123),
            ..StoredConnectionSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("apiHash"));
        assert!(!json.contains("api_hash"));
    }

    #[test]
    fn rejects_malformed_api_hashes() {
        assert!(validate_api_hash("not-a-secret").is_err());
        assert!(validate_api_hash("0123456789abcdef0123456789abcdef").is_ok());
    }

    #[test]
    fn legacy_demo_settings_are_treated_as_unconfigured() {
        let stored: StoredConnectionSettings = serde_json::from_str(
            r#"{"schemaVersion":1,"setupComplete":true,"runtimeMode":"demo"}"#,
        )
        .unwrap();
        assert!(!public_setup_complete(&stored));
    }
}
