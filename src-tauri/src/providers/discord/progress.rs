//! Latest-value diagnostics contain counters and closed states, never source data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscordImportPhase {
    Inspecting,
    Hashing,
    Registering,
    Importing,
    Verifying,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscordImportProgress {
    pub phase: DiscordImportPhase,
    pub inventory_entries: Option<u64>,
    pub processed_entries: u64,
    pub total_bytes: Option<u64>,
    /// Cumulative bytes across both full-file hash passes.
    pub hashed_bytes: u64,
    /// Physical reads made by ZIP inspection and typed parsing; may reread bytes.
    pub read_bytes: u64,
    pub parsed_records: u64,
    pub total_records: Option<u64>,
    pub committed_items: u64,
    pub committed_bytes: u64,
    pub committed_batches: u64,
    pub warnings: u64,
}
impl Default for DiscordImportProgress {
    fn default() -> Self {
        Self {
            phase: DiscordImportPhase::Inspecting,
            inventory_entries: None,
            processed_entries: 0,
            total_bytes: None,
            hashed_bytes: 0,
            read_bytes: 0,
            parsed_records: 0,
            total_records: None,
            committed_items: 0,
            committed_bytes: 0,
            committed_batches: 0,
            warnings: 0,
        }
    }
}
