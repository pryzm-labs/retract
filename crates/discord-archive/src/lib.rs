//! Bounded, format-neutral archive inspection. No extraction or runtime services.
mod error;
mod inventory;
mod limits;
mod profile;
mod reader;
mod record;
mod structure;

pub use error::ArchiveError;
pub use inventory::{ArchiveInventory, Cancellation, EntryIndex};
pub use limits::ArchiveLimits;
pub use profile::{
    AccountHeader, ContextHeader, ContextInspection, DiscordProfile, GuildHeader, ProfileInspection,
};
pub use reader::DiscordArchiveReader;
pub use record::{
    ChannelContext, DiscordId, EntryIntegrity, ExportAccount, GuildContext, ReadSummary,
    RecordSink, SentMessage,
};
pub use structure::{
    DecimalGrammar, EntryStructure, GrammarSet, JsonShape, NodeSummary, PathToken, ScalarGrammars,
    StructureProbe, StructureReport, TimestampGrammar, TimestampPrecision, TimestampSeparator,
    TimestampZone, TypeCounts,
};
