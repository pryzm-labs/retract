use crate::ArchiveError;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Callers may lower ceilings, but cannot increase the reviewed design bounds.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub max_archive_bytes: u64,
    pub max_directory_bytes: u64,
    pub max_entries: u64,
    pub max_path_bytes: u64,
    pub max_path_components: u64,
    pub max_entry_bytes: u64,
    pub max_total_declared_bytes: u64,
    pub max_observed_bytes: u64,
    pub max_expansion_ratio: u64,
    pub max_json_depth: u64,
    pub max_scalar_bytes: u64,
    pub max_raw_record_bytes: u64,
    pub max_decoded_record_bytes: u64,
    pub max_json_tokens: u64,
    pub max_display_bytes: u64,
    pub max_selected_contexts: u64,
    pub max_structure_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 4 * GIB,
            max_directory_bytes: 32 * MIB,
            max_entries: 100_000,
            max_path_bytes: 1024,
            max_path_components: 16,
            max_entry_bytes: 2 * GIB,
            max_total_declared_bytes: 16 * GIB,
            max_observed_bytes: 4 * GIB,
            max_expansion_ratio: 1000,
            max_json_depth: 64,
            max_scalar_bytes: MIB,
            max_raw_record_bytes: 8 * MIB,
            max_decoded_record_bytes: 2 * MIB,
            // A structural probe retains field names; bound that work too.
            max_json_tokens: 1_000_000,
            max_display_bytes: 4096,
            max_selected_contexts: 40_000,
            max_structure_bytes: 32 * MIB,
        }
    }
}

impl ArchiveLimits {
    pub(crate) fn validate(self) -> Result<(), ArchiveError> {
        let ceiling = Self::default();
        macro_rules! bounded {
            ($($field:ident),+ $(,)?) => {
                $(if self.$field == 0 || self.$field > ceiling.$field {
                    return Err(ArchiveError::InvalidLimits);
                })+
            };
        }
        bounded!(
            max_archive_bytes,
            max_directory_bytes,
            max_entries,
            max_path_bytes,
            max_path_components,
            max_entry_bytes,
            max_total_declared_bytes,
            max_observed_bytes,
            max_expansion_ratio,
            max_json_depth,
            max_scalar_bytes,
            max_raw_record_bytes,
            max_decoded_record_bytes,
            max_json_tokens,
            max_display_bytes,
            max_selected_contexts,
            max_structure_bytes
        );
        Ok(())
    }
}

pub(crate) fn add(left: u64, right: u64) -> Result<u64, ArchiveError> {
    left.checked_add(right).ok_or(ArchiveError::LimitExceeded)
}

pub(crate) fn bounded(value: u64, maximum: u64) -> Result<(), ArchiveError> {
    if value > maximum {
        Err(ArchiveError::LimitExceeded)
    } else {
        Ok(())
    }
}

pub(crate) fn ratio(expanded: u64, compressed: u64, maximum: u64) -> Result<(), ArchiveError> {
    let allowed = compressed
        .checked_mul(maximum)
        .ok_or(ArchiveError::LimitExceeded)?;
    bounded(expanded, allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every independently configurable ceiling must reject both disabling
    // and raising it. The integration suites exercise lower injected values.
    #[test]
    fn every_production_ceiling_is_positive_and_only_lowerable() {
        let defaults = ArchiveLimits::default();
        macro_rules! check {
            ($($field:ident),+ $(,)?) => { $(
                let mut lower = defaults;
                lower.$field = 1;
                assert_eq!(lower.validate(), Ok(()), stringify!($field));
                lower.$field = 0;
                assert_eq!(lower.validate(), Err(ArchiveError::InvalidLimits), stringify!($field));
                lower.$field = defaults.$field + 1;
                assert_eq!(lower.validate(), Err(ArchiveError::InvalidLimits), stringify!($field));
            )+ };
        }
        check!(
            max_archive_bytes,
            max_directory_bytes,
            max_entries,
            max_path_bytes,
            max_path_components,
            max_entry_bytes,
            max_total_declared_bytes,
            max_observed_bytes,
            max_expansion_ratio,
            max_json_depth,
            max_scalar_bytes,
            max_raw_record_bytes,
            max_decoded_record_bytes,
            max_json_tokens,
            max_display_bytes,
            max_selected_contexts,
            max_structure_bytes
        );
    }
}
