//! Bounded typed source records. Debug deliberately omits all source values.
use crate::{
    ArchiveError, DecimalGrammar, EntryIndex, TimestampGrammar, TimestampPrecision,
    TimestampSeparator, TimestampZone,
    structure::{decimal_grammar, timestamp_grammar},
};
use std::fmt;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct DiscordId(String);
impl DiscordId {
    pub fn parse(value: &str) -> Result<Self, ArchiveError> {
        if decimal_grammar(value) != DecimalGrammar::CanonicalPositiveU64Decimal {
            return Err(ArchiveError::InvalidProfile);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Eq, PartialEq)]
pub struct ExportAccount {
    pub id: DiscordId,
    pub username: String,
}
#[derive(Eq, PartialEq)]
pub struct GuildContext {
    pub id: DiscordId,
    pub name: String,
}
#[derive(Eq, PartialEq)]
pub struct ChannelContext {
    pub id: DiscordId,
    pub source_type: String,
    pub name: Option<String>,
    pub recipients: Option<Vec<String>>,
    pub guild: Option<GuildContext>,
}
#[derive(Eq, PartialEq)]
pub struct SentMessage {
    pub id: DiscordId,
    pub account_id: DiscordId,
    pub channel_id: DiscordId,
    /// UTC milliseconds validated against the message's Snowflake.
    pub timestamp_millis: i64,
    pub contents: String,
    /// The exact opaque attachment encoding; never split or fetched here.
    pub attachments: String,
}
/// Emitted only once parsing, decoder EOF, size and CRC checks all succeed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryIntegrity {
    pub entry: EntryIndex,
    pub messages: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadSummary {
    pub channels: u64,
    pub messages: u64,
}

/// Synchronous return provides backpressure. Any sink error aborts the pass;
/// messages accepted before an error are provisional, never entry completion.
pub trait RecordSink {
    fn begin_channel(&mut self, channel: ChannelContext) -> Result<(), ArchiveError>;
    fn message(&mut self, message: SentMessage) -> Result<(), ArchiveError>;
    fn end_channel(&mut self, integrity: EntryIntegrity) -> Result<(), ArchiveError>;
}
macro_rules! redacted_debug { ($($ty:ty),+) => {$(impl fmt::Debug for $ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($ty), " { redacted }")) }
})+}; }
redacted_debug!(
    DiscordId,
    ExportAccount,
    GuildContext,
    ChannelContext,
    SentMessage
);

pub(crate) fn timestamp_millis(id: &DiscordId, timestamp: &str) -> Result<i64, ArchiveError> {
    if timestamp_grammar(timestamp)
        != (TimestampGrammar::Calendar {
            separator: TimestampSeparator::Space,
            precision: TimestampPrecision::Seconds,
            zone: TimestampZone::Unzoned,
        })
    {
        return Err(ArchiveError::InvalidProfile);
    }
    // Only validated ASCII calendar digits reach the arithmetic below. Gregorian
    // days before this date are independent of host timezone and clock state.
    let part = |start: usize, end: usize| {
        timestamp[start..end]
            .parse::<i64>()
            .map_err(|_| ArchiveError::InvalidProfile)
    };
    let year = part(0, 4)?;
    let month = part(5, 7)?;
    let day = part(8, 10)?;
    let previous = year - 1;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    const MONTH_DAYS: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let days = previous * 365 + previous / 4 - previous / 100
        + previous / 400
        + MONTH_DAYS[(month - 1) as usize]
        + i64::from(leap && month > 2)
        + day
        - 1
        - 719_162;
    let seconds = days
        .checked_mul(86_400)
        .and_then(|s| {
            s.checked_add(part(11, 13).ok()? * 3600 + part(14, 16).ok()? * 60 + part(17, 19).ok()?)
        })
        .ok_or(ArchiveError::InvalidProfile)?;
    // https://docs.discord.com/developers/reference#snowflakes
    let snowflake = id
        .as_str()
        .parse::<u64>()
        .map_err(|_| ArchiveError::InvalidProfile)?;
    let millis = (snowflake >> 22)
        .checked_add(1_420_070_400_000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(ArchiveError::InvalidProfile)?;
    if millis / 1000 != seconds {
        return Err(ArchiveError::InvalidProfile);
    }
    Ok(millis)
}
