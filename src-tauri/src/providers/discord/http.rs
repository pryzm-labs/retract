use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

use super::session::valid_user_token;

const DISCORD_API: &str = "https://discord.com/api/v10/";
const MAX_RATE_LIMIT_BODY: usize = 4096;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteOutcome {
    Deleted,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscordDeleteError {
    Authentication,
    Permission,
    RateLimited {
        retry_after_millis: u64,
        global: bool,
    },
    Transient,
    Ambiguous,
    Permanent,
    InvalidTarget,
    Cancelled,
}

pub(crate) struct DiscordDeleteClient {
    client: reqwest::Client,
    base: url::Url,
    minimum_channel_interval: Duration,
    minimum_account_interval: Duration,
    channel_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    channel_deadlines: Mutex<HashMap<String, Instant>>,
    account_deadline: Mutex<Option<Instant>>,
    global_deadline: Mutex<Option<Instant>>,
    concurrency: Semaphore,
}

impl DiscordDeleteClient {
    pub(crate) fn production() -> Result<Self, DiscordDeleteError> {
        Self::new(DISCORD_API, Duration::from_millis(1500), true)
    }

    #[cfg(test)]
    pub(crate) fn for_test(base: &str) -> Result<Self, DiscordDeleteError> {
        Self::new(base, Duration::ZERO, false)
    }

    fn new(
        base: &str,
        minimum_channel_interval: Duration,
        https_only: bool,
    ) -> Result<Self, DiscordDeleteError> {
        let base = url::Url::parse(base).map_err(|_| DiscordDeleteError::InvalidTarget)?;
        if (https_only && base.scheme() != "https")
            || base.cannot_be_a_base()
            || base.host_str().is_none()
        {
            return Err(DiscordDeleteError::InvalidTarget);
        }
        let client = reqwest::Client::builder()
            .https_only(https_only)
            .timeout(Duration::from_secs(20))
            .user_agent("Retract/0.1")
            .build()
            .map_err(|_| DiscordDeleteError::Transient)?;
        Ok(Self {
            client,
            base,
            minimum_channel_interval,
            minimum_account_interval: if minimum_channel_interval.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_millis(50)
            },
            channel_locks: Mutex::new(HashMap::new()),
            channel_deadlines: Mutex::new(HashMap::new()),
            account_deadline: Mutex::new(None),
            global_deadline: Mutex::new(None),
            concurrency: Semaphore::new(4),
        })
    }

    pub(crate) async fn delete(
        &self,
        channel_id: &str,
        message_id: &str,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<DeleteOutcome, DiscordDeleteError> {
        if !valid_id(channel_id) || !valid_id(message_id) || !valid_user_token(token) {
            return Err(DiscordDeleteError::InvalidTarget);
        }
        check_cancelled(cancelled)?;
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| DiscordDeleteError::Cancelled)?;
        let channel_lock = {
            let mut locks = self.channel_locks.lock().await;
            if locks.len() >= 4096 {
                locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            }
            locks
                .entry(channel_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _channel_guard = channel_lock.lock().await;
        self.wait_for_global(cancelled).await?;
        self.wait_for_account(cancelled).await?;
        self.wait_for_channel(channel_id, cancelled).await?;
        check_cancelled(cancelled)?;

        let endpoint = self
            .base
            .join(&format!("channels/{channel_id}/messages/{message_id}"))
            .map_err(|_| DiscordDeleteError::InvalidTarget)?;
        let response = self
            .client
            .delete(endpoint)
            .header(reqwest::header::AUTHORIZATION, token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| DiscordDeleteError::Ambiguous)?;
        match response.status().as_u16() {
            204 => Ok(DeleteOutcome::Deleted),
            404 => Ok(DeleteOutcome::AlreadyAbsent),
            401 => Err(DiscordDeleteError::Authentication),
            403 => Err(DiscordDeleteError::Permission),
            429 => self.rate_limit(response).await,
            500..=599 => Err(DiscordDeleteError::Transient),
            _ => Err(DiscordDeleteError::Permanent),
        }
    }

    async fn wait_for_global(&self, cancelled: &AtomicBool) -> Result<(), DiscordDeleteError> {
        let deadline = *self.global_deadline.lock().await;
        if let Some(deadline) = deadline {
            wait_until(deadline, cancelled).await?;
        }
        Ok(())
    }

    async fn wait_for_account(&self, cancelled: &AtomicBool) -> Result<(), DiscordDeleteError> {
        let deadline = *self.account_deadline.lock().await;
        if let Some(deadline) = deadline {
            wait_until(deadline, cancelled).await?;
        }
        *self.account_deadline.lock().await = Some(Instant::now() + self.minimum_account_interval);
        Ok(())
    }

    async fn wait_for_channel(
        &self,
        channel_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        let deadline = self.channel_deadlines.lock().await.get(channel_id).copied();
        if let Some(deadline) = deadline {
            wait_until(deadline, cancelled).await?;
        }
        self.channel_deadlines.lock().await.insert(
            channel_id.to_owned(),
            Instant::now() + self.minimum_channel_interval,
        );
        Ok(())
    }

    async fn rate_limit(
        &self,
        response: reqwest::Response,
    ) -> Result<DeleteOutcome, DiscordDeleteError> {
        let bytes = response
            .bytes()
            .await
            .map_err(|_| DiscordDeleteError::Transient)?;
        if bytes.len() > MAX_RATE_LIMIT_BODY {
            return Err(DiscordDeleteError::Transient);
        }
        #[derive(Deserialize)]
        struct RateLimit {
            retry_after: f64,
            #[serde(default)]
            global: bool,
        }
        let limit: RateLimit =
            serde_json::from_slice(&bytes).map_err(|_| DiscordDeleteError::Transient)?;
        if !limit.retry_after.is_finite() || limit.retry_after <= 0.0 {
            return Err(DiscordDeleteError::Transient);
        }
        let duration = Duration::from_secs_f64(limit.retry_after).min(MAX_RETRY_AFTER);
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        if limit.global {
            *self.global_deadline.lock().await = Some(Instant::now() + duration);
        }
        Err(DiscordDeleteError::RateLimited {
            retry_after_millis: millis,
            global: limit.global,
        })
    }
}

fn valid_id(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|parsed| parsed > 0) && !value.starts_with('0')
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), DiscordDeleteError> {
    if cancelled.load(Ordering::Acquire) {
        Err(DiscordDeleteError::Cancelled)
    } else {
        Ok(())
    }
}

async fn wait_until(deadline: Instant, cancelled: &AtomicBool) -> Result<(), DiscordDeleteError> {
    loop {
        check_cancelled(cancelled)?;
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(());
        };
        tokio::time::sleep(remaining.min(Duration::from_millis(200))).await;
    }
}
