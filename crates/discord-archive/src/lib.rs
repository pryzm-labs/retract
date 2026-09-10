//! Bounded, format-neutral archive inspection. No extraction or runtime services.
mod error;
mod inventory;
mod limits;
mod profile;
mod structure;

pub use error::ArchiveError;
pub use inventory::{ArchiveInventory, Cancellation, EntryIndex};
pub use limits::ArchiveLimits;
pub use profile::{
    AccountHeader, ContextHeader, ContextInspection, DiscordProfile, GuildHeader, ProfileInspection,
};
pub use structure::{
    DecimalGrammar, GrammarSet, JsonShape, PathToken, ScalarGrammars, StructureProbe,
    StructureReport, TimestampGrammar, TimestampPrecision, TimestampSeparator, TimestampZone,
};
