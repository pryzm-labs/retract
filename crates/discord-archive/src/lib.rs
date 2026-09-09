//! Bounded, format-neutral archive inspection. No extraction or runtime services.
mod error;
mod inventory;
mod limits;
mod structure;

pub use error::ArchiveError;
pub use inventory::{ArchiveInventory, Cancellation, EntryIndex};
pub use limits::ArchiveLimits;
pub use structure::{JsonShape, StructureProbe, StructureReport};
