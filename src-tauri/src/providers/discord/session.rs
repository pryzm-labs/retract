use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeroize::Zeroizing;

use crate::error::AppError;

const DISCORD_CURRENT_USER_ENDPOINT: &str = "https://discord.com/api/v10/users/@me";
const MAX_IDENTITY_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) struct StoredDiscordCredential {
    account_id: String,
    token: Zeroizing<String>,
}

impl StoredDiscordCredential {
    pub(crate) fn new(account_id: String, token: Zeroizing<String>) -> Result<Self, AppError> {
        if !valid_account_id(&account_id) || !valid_user_token(&token) {
            return Err(invalid_credential());
        }
        Ok(Self { account_id, token })
    }

    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn expose_token_for_request<T>(&self, operation: impl FnOnce(&str) -> T) -> T {
        operation(&self.token)
    }
}

pub(crate) trait DiscordCredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, AppError>;
    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), AppError>;
    fn forget(&self) -> Result<(), AppError>;
}

pub(crate) struct SystemDiscordCredentialStore;

impl DiscordCredentialStore for SystemDiscordCredentialStore {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, AppError> {
        crate::secure_store::load_discord_credential()?
            .map(|credential| StoredDiscordCredential::new(credential.account_id, credential.token))
            .transpose()
    }

    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), AppError> {
        credential.expose_token_for_request(|token| {
            crate::secure_store::save_discord_credential(credential.account_id(), token)
        })
    }

    fn forget(&self) -> Result<(), AppError> {
        crate::secure_store::forget_discord_credential()
    }
}

#[derive(Clone)]
pub(crate) struct VerifiedDiscordIdentity {
    pub(crate) account_id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
}

#[async_trait]
pub(crate) trait DiscordIdentityClient: Send + Sync {
    async fn verify(&self, token: &str) -> Result<VerifiedDiscordIdentity, AppError>;
}

pub(crate) struct ReqwestDiscordIdentityClient {
    client: reqwest::Client,
}

impl ReqwestDiscordIdentityClient {
    fn new() -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(15))
            .user_agent("Retract/0.1")
            .build()
            .map_err(|_| verification_failed())?;
        Ok(Self { client })
    }
}

#[derive(Deserialize)]
struct DiscordCurrentUser {
    id: String,
    username: String,
    global_name: Option<String>,
}

#[async_trait]
impl DiscordIdentityClient for ReqwestDiscordIdentityClient {
    async fn verify(&self, token: &str) -> Result<VerifiedDiscordIdentity, AppError> {
        let response = self
            .client
            .get(DISCORD_CURRENT_USER_ENDPOINT)
            .header(reqwest::header::AUTHORIZATION, token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| verification_failed())?;
        if !response.status().is_success() {
            return Err(verification_failed());
        }

        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| verification_failed())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_IDENTITY_RESPONSE_BYTES {
                return Err(verification_failed());
            }
            bytes.extend_from_slice(&chunk);
        }
        let current: DiscordCurrentUser =
            serde_json::from_slice(&bytes).map_err(|_| verification_failed())?;
        if !valid_account_id(&current.id) || current.username.trim().is_empty() {
            return Err(verification_failed());
        }
        Ok(VerifiedDiscordIdentity {
            account_id: current.id,
            display_name: current
                .global_name
                .unwrap_or_else(|| current.username.clone()),
            username: current.username,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscordSessionState {
    Disconnected,
    Verifying,
    Ready,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscordSessionStatus {
    pub(crate) state: DiscordSessionState,
    pub(crate) account_id: Option<String>,
    pub(crate) username: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) remembered: bool,
}

struct DiscordSessionData {
    state: DiscordSessionState,
    credential: Option<StoredDiscordCredential>,
    identity: Option<VerifiedDiscordIdentity>,
    remembered: bool,
}

impl Default for DiscordSessionData {
    fn default() -> Self {
        Self {
            state: DiscordSessionState::Disconnected,
            credential: None,
            identity: None,
            remembered: false,
        }
    }
}

pub(crate) struct DiscordSessionOwner {
    data: Mutex<DiscordSessionData>,
    identity_client: Arc<dyn DiscordIdentityClient>,
    credential_store: Arc<dyn DiscordCredentialStore>,
}

impl DiscordSessionOwner {
    pub(crate) fn production() -> Result<Self, AppError> {
        Ok(Self::with_dependencies(
            Arc::new(ReqwestDiscordIdentityClient::new()?),
            Arc::new(SystemDiscordCredentialStore),
        ))
    }

    pub(crate) fn with_dependencies(
        identity_client: Arc<dyn DiscordIdentityClient>,
        credential_store: Arc<dyn DiscordCredentialStore>,
    ) -> Self {
        Self {
            data: Mutex::new(DiscordSessionData::default()),
            identity_client,
            credential_store,
        }
    }

    pub(crate) fn status(&self) -> DiscordSessionStatus {
        let Ok(data) = self.data.lock() else {
            return disconnected_status();
        };
        status_from(&data)
    }

    pub(crate) async fn submit_manual(
        &self,
        expected_account_id: &str,
        token: &str,
        remember: bool,
    ) -> Result<DiscordSessionStatus, AppError> {
        self.install(
            expected_account_id,
            Zeroizing::new(token.to_owned()),
            remember,
            true,
        )
        .await
    }

    pub(crate) async fn install_captured(
        &self,
        expected_account_id: &str,
        token: Zeroizing<String>,
        remember: bool,
    ) -> Result<DiscordSessionStatus, AppError> {
        self.install(expected_account_id, token, remember, true)
            .await
    }

    pub(crate) async fn load_remembered(
        &self,
        expected_account_id: &str,
    ) -> Result<DiscordSessionStatus, AppError> {
        let Some(credential) = self.credential_store.load()? else {
            return Ok(self.status());
        };
        if credential.account_id() != expected_account_id {
            return Err(account_mismatch());
        }
        let token = credential.expose_token_for_request(|value| Zeroizing::new(value.to_owned()));
        self.install(expected_account_id, token, true, false).await
    }

    async fn install(
        &self,
        expected_account_id: &str,
        token: Zeroizing<String>,
        remember: bool,
        persist: bool,
    ) -> Result<DiscordSessionStatus, AppError> {
        if !valid_account_id(expected_account_id) {
            return Err(invalid_credential());
        }
        let candidate = StoredDiscordCredential::new(expected_account_id.to_owned(), token)?;
        self.set_verifying()?;
        let request_token =
            candidate.expose_token_for_request(|value| Zeroizing::new(value.to_owned()));
        let identity = match self.identity_client.verify(&request_token).await {
            Ok(identity) if identity.account_id == expected_account_id => identity,
            Ok(_) => {
                self.disconnect();
                return Err(account_mismatch());
            }
            Err(error) => {
                self.disconnect();
                return Err(error);
            }
        };
        if remember && persist {
            if let Err(error) = self.credential_store.save(&candidate) {
                self.disconnect();
                return Err(error);
            }
        }

        let mut data = self.data.lock().map_err(|_| AppError::StateUnavailable)?;
        data.state = DiscordSessionState::Ready;
        data.credential = Some(candidate);
        data.identity = Some(identity);
        data.remembered = remember;
        Ok(status_from(&data))
    }

    pub(crate) fn with_token<T>(
        &self,
        expected_account_id: &str,
        operation: impl FnOnce(&str) -> T,
    ) -> Result<T, AppError> {
        let data = self.data.lock().map_err(|_| AppError::StateUnavailable)?;
        let credential = data.credential.as_ref().ok_or_else(session_required)?;
        if data.state != DiscordSessionState::Ready
            || credential.account_id() != expected_account_id
        {
            return Err(session_required());
        }
        Ok(credential.expose_token_for_request(operation))
    }

    pub(crate) fn forget(&self) -> Result<(), AppError> {
        self.disconnect();
        self.credential_store.forget()
    }

    pub(crate) fn shutdown(&self) {
        self.disconnect();
    }

    fn set_verifying(&self) -> Result<(), AppError> {
        let mut data = self.data.lock().map_err(|_| AppError::StateUnavailable)?;
        data.credential = None;
        data.identity = None;
        data.remembered = false;
        data.state = DiscordSessionState::Verifying;
        Ok(())
    }

    fn disconnect(&self) {
        if let Ok(mut data) = self.data.lock() {
            *data = DiscordSessionData::default();
        }
    }
}

fn status_from(data: &DiscordSessionData) -> DiscordSessionStatus {
    DiscordSessionStatus {
        state: data.state,
        account_id: data
            .identity
            .as_ref()
            .map(|identity| identity.account_id.clone()),
        username: data
            .identity
            .as_ref()
            .map(|identity| identity.username.clone()),
        display_name: data
            .identity
            .as_ref()
            .map(|identity| identity.display_name.clone()),
        remembered: data.remembered,
    }
}

fn disconnected_status() -> DiscordSessionStatus {
    status_from(&DiscordSessionData::default())
}

fn valid_account_id(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|parsed| parsed > 0) && !value.starts_with('0')
}

fn valid_user_token(value: &str) -> bool {
    (20..=MAX_TOKEN_BYTES).contains(&value.len())
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_whitespace() && !byte.is_ascii_control())
        && !value.starts_with("Bot ")
        && !value.starts_with("Bearer ")
}

fn invalid_credential() -> AppError {
    AppError::InvalidRequest("Discord user credential is malformed".into())
}

fn verification_failed() -> AppError {
    AppError::InvalidRequest("Discord session could not be verified".into())
}

fn account_mismatch() -> AppError {
    AppError::InvalidRequest("Discord account does not match the imported archive".into())
}

fn session_required() -> AppError {
    AppError::InvalidRequest("A matching Discord session is required".into())
}
