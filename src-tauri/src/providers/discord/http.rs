use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;

use super::session::valid_user_token;

const DISCORD_API: &str = "https://discord.com/api/v10/";
const MAX_RATE_LIMIT_BODY: usize = 4096;
const MAX_LIVE_MESSAGE_BODY: usize = 64 * 1024;
pub(super) const MAX_JOB_WAIT_MILLIS: u64 = 300_000;
const MAX_TRANSIENT_ATTEMPTS: usize = 3;

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
        retry_after_millis: Option<u64>,
        global: bool,
    },
    Transient,
    Ambiguous,
    Permanent,
    InvalidTarget,
    NotFound,
    OwnershipMismatch,
    Cancelled,
}

#[derive(Deserialize)]
struct LiveMessageAuthor {
    id: String,
}

#[derive(Deserialize)]
struct LiveMessageAttachment {
    filename: String,
}

#[derive(Deserialize)]
struct LiveMessage {
    id: String,
    channel_id: String,
    author: LiveMessageAuthor,
    content: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    attachments: Vec<LiveMessageAttachment>,
}

/// Content-free, transient proof of the archive message the user reviewed.
/// This value is never serialized into plans or job records.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DiscordLiveMessageBinding {
    reviewed_sha256: [u8; 32],
    timestamp_millis: i64,
}

impl DiscordLiveMessageBinding {
    pub(crate) fn new(content: &str, timestamp_millis: i64) -> Self {
        Self::with_attachment_names(content, timestamp_millis, &[])
    }

    pub(crate) fn with_attachment_names(
        content: &str,
        timestamp_millis: i64,
        attachment_names: &[Option<&str>],
    ) -> Self {
        Self {
            reviewed_sha256: reviewed_digest(content, attachment_names),
            timestamp_millis,
        }
    }

    fn matches(&self, message: &LiveMessage) -> bool {
        let attachment_names = message
            .attachments
            .iter()
            .map(|attachment| Some(attachment.filename.as_str()))
            .collect::<Vec<_>>();
        self.timestamp_millis == message.timestamp.timestamp_millis()
            && self.reviewed_sha256 == reviewed_digest(&message.content, &attachment_names)
    }
}

fn reviewed_digest(content: &str, attachment_names: &[Option<&str>]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"retract.discord.reviewed-message.v1\0");
    digest.update(
        u64::try_from(content.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    digest.update(content.as_bytes());
    digest.update(
        u64::try_from(attachment_names.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for name in attachment_names {
        match name {
            Some(name) => {
                digest.update([1]);
                digest.update(u64::try_from(name.len()).unwrap_or(u64::MAX).to_be_bytes());
                digest.update(name.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    digest.finalize().into()
}

struct LiveMessageExpectation<'a> {
    channel_id: &'a str,
    message_id: &'a str,
    owner_user_id: &'a str,
    binding: &'a DiscordLiveMessageBinding,
}

/// An unrepresentable delay has no safe retry time for this client.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RetryDeadline {
    Until(Instant),
    Indefinite,
}

#[derive(Default)]
struct RateLimits {
    global: Option<RetryDeadline>,
    channels: HashMap<String, RetryDeadline>,
}

impl RateLimits {
    fn record(&mut self, channel: &str, global: bool, deadline: RetryDeadline, now: Instant) {
        self.channels
            .retain(|_, end| *end > RetryDeadline::Until(now));
        if global {
            self.global = self.global.max(Some(deadline));
        } else {
            self.channels
                .entry(channel.to_owned())
                .and_modify(|old| *old = (*old).max(deadline))
                .or_insert(deadline);
        }
    }

    fn check(&self, channel: &str, now: Instant) -> Result<(), DiscordDeleteError> {
        let channel_deadline = self.channels.get(channel).copied();
        let Some(deadline) = self.global.max(channel_deadline) else {
            return Ok(());
        };
        let retry_after_millis = match deadline {
            RetryDeadline::Until(end) if end <= now => return Ok(()),
            RetryDeadline::Until(end) => {
                u64::try_from((end - now).as_nanos().div_ceil(1_000_000)).ok()
            }
            RetryDeadline::Indefinite => None,
        };
        Err(DiscordDeleteError::RateLimited {
            retry_after_millis,
            global: self.global == Some(deadline),
        })
    }
}

pub(crate) struct DiscordDeleteClient {
    client: reqwest::Client,
    base: url::Url,
    minimum_channel_interval: Duration,
    minimum_account_interval: Duration,
    transient_retry_base: Duration,
    channel_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    channel_deadlines: Mutex<HashMap<String, Instant>>,
    account_deadline: Mutex<Option<Instant>>,
    rate_limits: Mutex<RateLimits>,
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
            transient_retry_base: if minimum_channel_interval.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_millis(500)
            },
            channel_locks: Mutex::new(HashMap::new()),
            channel_deadlines: Mutex::new(HashMap::new()),
            account_deadline: Mutex::new(None),
            rate_limits: Mutex::default(),
            concurrency: Semaphore::new(4),
        })
    }

    pub(crate) async fn verify_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        self.validate_target(channel_id, message_id, owner_user_id, token, cancelled)?;
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| DiscordDeleteError::Cancelled)?;
        let channel_lock = self.channel_lock(channel_id).await;
        let _channel_guard = channel_lock.lock().await;
        self.wait_for_account(cancelled).await?;
        self.wait_for_channel(channel_id, cancelled).await?;
        let endpoint = self.message_endpoint(channel_id, message_id)?;
        let expected = LiveMessageExpectation {
            channel_id,
            message_id,
            owner_user_id,
            binding: expected,
        };
        self.request_ownership(endpoint, &expected, token, cancelled)
            .await
    }

    pub(crate) async fn delete_owned(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        expected: &DiscordLiveMessageBinding,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<DeleteOutcome, DiscordDeleteError> {
        self.validate_target(channel_id, message_id, owner_user_id, token, cancelled)?;
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| DiscordDeleteError::Cancelled)?;
        let channel_lock = self.channel_lock(channel_id).await;
        let _channel_guard = channel_lock.lock().await;
        let endpoint = self.message_endpoint(channel_id, message_id)?;
        let expected = LiveMessageExpectation {
            channel_id,
            message_id,
            owner_user_id,
            binding: expected,
        };
        for attempt in 0..MAX_TRANSIENT_ATTEMPTS {
            self.wait_for_account(cancelled).await?;
            self.wait_for_channel(channel_id, cancelled).await?;
            check_cancelled(cancelled)?;

            match self
                .request_ownership(endpoint.clone(), &expected, token, cancelled)
                .await
            {
                Ok(()) => {}
                Err(DiscordDeleteError::Transient) if attempt + 1 < MAX_TRANSIENT_ATTEMPTS => {
                    self.wait_for_transient_retry(attempt, cancelled).await?;
                    continue;
                }
                Err(error) => return Err(error),
            }
            self.check_rate_limit(channel_id, cancelled).await?;
            let response = self
                .client
                .delete(endpoint.clone())
                .header(reqwest::header::AUTHORIZATION, token)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await;
            match response {
                Ok(response) => match response.status().as_u16() {
                    204 => return Ok(DeleteOutcome::Deleted),
                    404 => return Ok(DeleteOutcome::AlreadyAbsent),
                    401 => return Err(DiscordDeleteError::Authentication),
                    403 => return Err(DiscordDeleteError::Permission),
                    429 => return Err(self.rate_limit(channel_id, response, cancelled).await),
                    500..=599 if attempt + 1 < MAX_TRANSIENT_ATTEMPTS => {}
                    500..=599 => return Err(DiscordDeleteError::Ambiguous),
                    _ => return Err(DiscordDeleteError::Permanent),
                },
                Err(_) if attempt + 1 < MAX_TRANSIENT_ATTEMPTS => {}
                Err(_) => return Err(DiscordDeleteError::Ambiguous),
            }
            self.wait_for_transient_retry(attempt, cancelled).await?;
        }
        Err(DiscordDeleteError::Ambiguous)
    }

    fn validate_target(
        &self,
        channel_id: &str,
        message_id: &str,
        owner_user_id: &str,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        if !valid_id(channel_id)
            || !valid_id(message_id)
            || !valid_id(owner_user_id)
            || !valid_user_token(token)
        {
            return Err(DiscordDeleteError::InvalidTarget);
        }
        check_cancelled(cancelled)
    }

    fn message_endpoint(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<url::Url, DiscordDeleteError> {
        self.base
            .join(&format!("channels/{channel_id}/messages/{message_id}"))
            .map_err(|_| DiscordDeleteError::InvalidTarget)
    }

    async fn channel_lock(&self, channel_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.channel_locks.lock().await;
        if locks.len() >= 4096 {
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        locks
            .entry(channel_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn request_ownership(
        &self,
        endpoint: url::Url,
        expected: &LiveMessageExpectation<'_>,
        token: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        self.check_rate_limit(expected.channel_id, cancelled)
            .await?;
        let response = self
            .client
            .get(endpoint)
            .header(reqwest::header::AUTHORIZATION, token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| DiscordDeleteError::Transient)?;
        if response.status().as_u16() == 429 {
            return Err(self
                .rate_limit(expected.channel_id, response, cancelled)
                .await);
        }
        check_cancelled(cancelled)?;
        match response.status().as_u16() {
            200 => {
                let bytes = bounded_body(response, MAX_LIVE_MESSAGE_BODY, cancelled).await?;
                let message: LiveMessage = serde_json::from_slice(&bytes)
                    .map_err(|_| DiscordDeleteError::OwnershipMismatch)?;
                if message.id != expected.message_id
                    || message.channel_id != expected.channel_id
                    || message.author.id != expected.owner_user_id
                    || !expected.binding.matches(&message)
                {
                    return Err(DiscordDeleteError::OwnershipMismatch);
                }
                Ok(())
            }
            401 => Err(DiscordDeleteError::Authentication),
            403 => Err(DiscordDeleteError::Permission),
            404 => Err(DiscordDeleteError::NotFound),
            500..=599 => Err(DiscordDeleteError::Transient),
            _ => Err(DiscordDeleteError::Permanent),
        }
    }

    async fn check_rate_limit(
        &self,
        channel: &str,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        let limits = self.rate_limits.lock().await;
        check_cancelled(cancelled)?;
        limits.check(channel, Instant::now())
    }

    async fn wait_for_account(&self, cancelled: &AtomicBool) -> Result<(), DiscordDeleteError> {
        let mut deadline = self.account_deadline.lock().await;
        if let Some(deadline) = *deadline {
            wait_until(deadline, cancelled).await?;
        }
        *deadline = Some(Instant::now() + self.minimum_account_interval);
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
        let jitter = if self.minimum_channel_interval.is_zero() {
            Duration::ZERO
        } else {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            Duration::from_millis(u64::from(nanos % 251))
        };
        self.channel_deadlines.lock().await.insert(
            channel_id.to_owned(),
            Instant::now() + self.minimum_channel_interval + jitter,
        );
        Ok(())
    }

    async fn wait_for_transient_retry(
        &self,
        attempt: usize,
        cancelled: &AtomicBool,
    ) -> Result<(), DiscordDeleteError> {
        let multiplier = 1_u32 << u32::try_from(attempt).unwrap_or(0).min(8);
        wait_duration(self.transient_retry_base * multiplier, cancelled).await
    }

    async fn rate_limit(
        &self,
        channel: &str,
        response: reqwest::Response,
        cancelled: &AtomicBool,
    ) -> DiscordDeleteError {
        #[derive(Deserialize)]
        struct RateLimit {
            retry_after: f64,
            #[serde(default)]
            global: bool,
        }
        let parsed = match bounded_body(response, MAX_RATE_LIMIT_BODY, cancelled).await {
            Ok(bytes) => serde_json::from_slice::<RateLimit>(&bytes).ok(),
            Err(_) => None,
        };
        // An unreadable response has unknown scope. Apply it to the whole client.
        let global = parsed.as_ref().is_none_or(|limit| limit.global);
        let now = Instant::now();
        let deadline = parsed
            .and_then(|limit| {
                let millis = (limit.retry_after * 1000.0).ceil();
                if !(millis > 0.0 && millis < u64::MAX as f64) {
                    return None;
                }
                now.checked_add(Duration::from_millis(millis as u64))
            })
            .map_or(RetryDeadline::Indefinite, RetryDeadline::Until);
        let mut limits = self.rate_limits.lock().await;
        limits.record(channel, global, deadline, now);
        if cancelled.load(Ordering::Acquire) {
            return DiscordDeleteError::Cancelled;
        }
        limits
            .check(channel, now)
            .expect_err("a new rate limit blocks requests")
    }
}

async fn bounded_body(
    response: reqwest::Response,
    maximum: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, DiscordDeleteError> {
    let content_length = response.content_length();
    if content_length.is_some_and(|length| length > maximum as u64) {
        return Err(DiscordDeleteError::Transient);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::with_capacity(
        content_length
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0),
    );
    while let Some(chunk) = stream.next().await {
        check_cancelled(cancelled)?;
        let chunk = chunk.map_err(|_| DiscordDeleteError::Transient)?;
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err(DiscordDeleteError::Transient);
        }
        bytes.extend_from_slice(&chunk);
    }
    check_cancelled(cancelled)?;
    Ok(bytes)
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
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(200))).await;
    }
}

async fn wait_duration(
    duration: Duration,
    cancelled: &AtomicBool,
) -> Result<(), DiscordDeleteError> {
    wait_until(Instant::now() + duration, cancelled).await
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    #[test]
    fn overlapping_deadlines_expire_only_at_the_full_provider_time() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut limits = RateLimits::default();
        limits.record("a", true, RetryDeadline::Until(at(600_000)), start);
        limits.record("b", true, RetryDeadline::Until(at(1_000)), start);
        limits.record("a", false, RetryDeadline::Until(at(601_000)), start);
        for (channel, now, remaining, global) in [
            ("b", 300_000, 300_000, true),
            ("a", 600_000, 1_000, false),
            ("a", 600_999, 1, false),
        ] {
            assert_eq!(
                limits.check(channel, at(now)),
                Err(DiscordDeleteError::RateLimited {
                    retry_after_millis: Some(remaining),
                    global,
                })
            );
        }
        assert_eq!(limits.check("b", at(600_000)), Ok(()));
        assert_eq!(limits.check("a", at(601_000)), Ok(()));
    }
}
