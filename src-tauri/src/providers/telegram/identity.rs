use std::sync::RwLock;

use chrono::Utc;
use retract_domain::{
    AccountId, AccountRecord, ActiveContext, ConnectionState, Scope, SourceId, SourceKind,
    SourceRecord, SourceState,
};
use uuid::Uuid;

use crate::{error::AppError, persistence::FoundationStore};

use super::locators::{
    TELEGRAM_ACCOUNT_SCHEMA, TELEGRAM_SCHEMA_VERSION, TelegramAccountLocator, TelegramEnvironment,
    TelegramSourceProfile, telegram_provider_key,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTelegramIdentity {
    pub environment: TelegramEnvironment,
    pub user_id: i64,
    pub session_generation: Uuid,
}

impl VerifiedTelegramIdentity {
    pub fn new(
        environment: TelegramEnvironment,
        user_id: i64,
        session_generation: Uuid,
    ) -> Result<Self, AppError> {
        if session_generation.is_nil() {
            return Err(invalid_identity());
        }
        TelegramAccountLocator::new(environment, user_id.to_string())?;
        Ok(Self {
            environment,
            user_id,
            session_generation,
        })
    }

    pub fn account_locator(&self) -> TelegramAccountLocator {
        TelegramAccountLocator::new(self.environment, self.user_id.to_string())
            .expect("verified Telegram identity retains a valid user ID")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramAccountProfile {
    pub display_name: String,
    pub username: Option<String>,
}

impl TelegramAccountProfile {
    fn validate(&self) -> Result<(), AppError> {
        if !bounded_display(&self.display_name, 512)
            || self
                .username
                .as_ref()
                .is_some_and(|username| !bounded_display(username, 256))
        {
            return Err(invalid_identity());
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct BindingState {
    expected_generation: Option<Uuid>,
    active: Option<ActiveContext>,
}

#[derive(Debug, Default)]
pub struct SessionBinding {
    state: RwLock<BindingState>,
}

impl SessionBinding {
    pub fn begin_generation(&self, generation: Uuid) -> Result<(), AppError> {
        if generation.is_nil() {
            return Err(invalid_identity());
        }
        let mut state = self.state.write().map_err(|_| AppError::StateUnavailable)?;
        if state.expected_generation != Some(generation) {
            state.expected_generation = Some(generation);
            state.active = None;
        }
        Ok(())
    }

    pub fn publish(
        &self,
        store: &FoundationStore,
        identity: &VerifiedTelegramIdentity,
        profile: &TelegramAccountProfile,
    ) -> Result<ActiveContext, AppError> {
        profile.validate()?;
        {
            let state = self.state.read().map_err(|_| AppError::StateUnavailable)?;
            if state.expected_generation != Some(identity.session_generation) {
                return Err(stale_generation());
            }
        }

        let account_locator = identity.account_locator();
        let now = Utc::now();
        let scope = store.transaction(|state| {
            let account_id = if let Some(account) = state
                .identities
                .iter_mut()
                .find(|account| telegram_account_matches(account, &account_locator))
            {
                account.display_name = profile.display_name.clone();
                account.username = profile.username.clone();
                account.connection_state = ConnectionState::Ready;
                account.last_seen_at = now;
                account.id
            } else {
                let id = AccountId::try_from(Uuid::new_v4()).map_err(|_| invalid_identity())?;
                state.identities.push(AccountRecord {
                    id,
                    provider: telegram_provider_key(),
                    native_identity: account_locator.clone().into_payload(),
                    display_name: profile.display_name.clone(),
                    username: profile.username.clone(),
                    avatar: None,
                    connection_state: ConnectionState::Ready,
                    created_at: now,
                    last_seen_at: now,
                });
                id
            };

            let source_id = if let Some(source) = state.sources.iter_mut().find(|source| {
                source.account_id == account_id
                    && source.provider == telegram_provider_key()
                    && source.kind == SourceKind::LiveConnection
            }) {
                source.state = SourceState::Ready;
                source.updated_at = now;
                source.id
            } else {
                let id = SourceId::try_from(Uuid::new_v4()).map_err(|_| invalid_identity())?;
                state.sources.push(SourceRecord {
                    id,
                    account_id,
                    provider: telegram_provider_key(),
                    kind: SourceKind::LiveConnection,
                    state: SourceState::Ready,
                    archive_fingerprint: None,
                    schema_profile: TelegramSourceProfile::payload(),
                    imported_at: None,
                    updated_at: now,
                    warnings: Vec::new(),
                });
                id
            };
            Ok(Scope {
                provider: telegram_provider_key(),
                account_id,
                source_id,
            })
        })?;

        let context = ActiveContext {
            scope,
            session_generation: identity.session_generation,
        };
        context.validate().map_err(|_| invalid_identity())?;
        let mut state = self.state.write().map_err(|_| AppError::StateUnavailable)?;
        if state.expected_generation != Some(identity.session_generation) {
            return Err(stale_generation());
        }
        state.active = Some(context.clone());
        Ok(context)
    }

    pub fn current(&self) -> Option<ActiveContext> {
        self.state
            .read()
            .ok()
            .and_then(|state| state.active.clone())
    }

    pub fn validate(&self, expected: &ActiveContext) -> Result<(), AppError> {
        expected.validate().map_err(|_| stale_generation())?;
        if self.current().as_ref() != Some(expected) {
            return Err(stale_generation());
        }
        Ok(())
    }

    pub fn invalidate(&self) {
        if let Ok(mut state) = self.state.write() {
            state.expected_generation = None;
            state.active = None;
        }
    }
}

fn telegram_account_matches(account: &AccountRecord, expected: &TelegramAccountLocator) -> bool {
    account.provider == telegram_provider_key()
        && account.native_identity.schema == TELEGRAM_ACCOUNT_SCHEMA
        && account.native_identity.version == TELEGRAM_SCHEMA_VERSION
        && serde_json::from_value::<TelegramAccountLocator>(account.native_identity.payload.clone())
            .is_ok_and(|actual| actual == *expected)
}

fn bounded_display(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn invalid_identity() -> AppError {
    AppError::InvalidRequest("Telegram account identity is invalid".into())
}

fn stale_generation() -> AppError {
    AppError::InvalidRequest("Telegram session generation changed".into())
}
