//! One exact, structural profile. No message decoding or timezone conversion.
use crate::{
    ArchiveError, ArchiveInventory, DecimalGrammar, EntryIndex, GrammarSet, JsonShape,
    StructureProbe, TimestampGrammar, TimestampPrecision, TimestampSeparator, TimestampZone,
    inventory::cancelled,
    limits::{add, bounded},
    structure::{EntryStructure, decimal_grammar, inspect_reader},
};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Seek},
};

pub struct AccountHeader {
    pub id: String,
    pub username: String,
}
pub struct GuildHeader {
    pub id: String,
    pub name: String,
}
/// `source_type` and recipients are opaque metadata, not semantic discriminators.
/// A name may be absent or null; neither establishes a conversation kind.
pub struct ContextHeader {
    pub id: String,
    pub source_type: String,
    pub name: Option<String>,
    pub recipients: Option<Vec<String>>,
    pub guild: Option<GuildHeader>,
}
pub struct ContextInspection {
    pub header_entry: EntryIndex,
    pub messages_entry: EntryIndex,
    pub header: ContextHeader,
}
pub struct ProfileInspection {
    pub schema_key: &'static str,
    pub schema_version: u32,
    pub policy_key: &'static str,
    pub account_entry: EntryIndex,
    pub index_entry: EntryIndex,
    pub account: AccountHeader,
    pub contexts: Vec<ContextInspection>,
}
// Callers must explicitly access typed values. Diagnostics never print them.
macro_rules! redacted_debug { ($($ty:ty),+) => {$(impl fmt::Debug for $ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($ty), " { redacted }")) }
})+}; }
redacted_debug!(
    AccountHeader,
    GuildHeader,
    ContextHeader,
    ContextInspection,
    ProfileInspection
);

pub struct DiscordProfile;
#[derive(Default)]
struct Pair {
    header: Option<EntryIndex>,
    messages: Option<EntryIndex>,
}

impl DiscordProfile {
    pub fn detect<R: Read + Seek>(
        archive: &mut ArchiveInventory<'_, R>,
    ) -> Result<ProfileInspection, ArchiveError> {
        let mut account = None;
        let mut index = None;
        let mut pairs: BTreeMap<String, Pair> = BTreeMap::new();
        let mut retained_headers = 0;
        let mut header_tokens = 0;
        for n in 0..archive.entry_count() {
            cancelled(archive.cancel)?;
            let entry = EntryIndex(n);
            let name = match archive.selected_name(entry) {
                Ok(name) => name,
                Err(ArchiveError::InvalidSelection) => continue, // directory
                Err(error) => return Err(error),
            };
            if name == "Account/user.json" {
                account = Some(entry);
                continue;
            }
            if name == "Messages/index.json" {
                index = Some(entry);
                continue;
            }
            let components: Vec<_> = name.split('/').collect();
            let filename = components.last().copied().unwrap_or_default();
            if filename.eq_ignore_ascii_case("user.json")
                || filename.eq_ignore_ascii_case("account.json")
                || filename.eq_ignore_ascii_case("messages.csv")
                || (filename.eq_ignore_ascii_case("index.json")
                    && components
                        .iter()
                        .any(|part| part.eq_ignore_ascii_case("Messages")))
            {
                return Err(ArchiveError::UnsupportedProfile);
            }
            if filename.eq_ignore_ascii_case("messages.json")
                || filename.eq_ignore_ascii_case("channel.json")
            {
                if components.len() != 3
                    || components[0] != "Messages"
                    || !matches!(filename, "channel.json" | "messages.json")
                {
                    return Err(ArchiveError::UnsupportedProfile);
                }
                let id = components[1]
                    .strip_prefix('c')
                    .ok_or(ArchiveError::UnsupportedProfile)?;
                valid_id(id)?;
                bounded(
                    pairs.len() as u64 + u64::from(!pairs.contains_key(id)),
                    archive.limits.max_selected_contexts,
                )?;
                if !pairs.contains_key(id) {
                    retained_headers = add(retained_headers, 512 + id.len() as u64 * 2)?;
                    bounded(retained_headers, archive.limits.max_structure_bytes)?;
                }
                let pair = pairs.entry(id.to_owned()).or_default();
                let slot = if filename == "channel.json" {
                    &mut pair.header
                } else {
                    &mut pair.messages
                };
                if slot.replace(entry).is_some() {
                    return Err(ArchiveError::InvalidProfile);
                }
            }
        }
        let account_entry = account.ok_or(ArchiveError::UnsupportedProfile)?;
        let index_entry = index.ok_or(ArchiveError::UnsupportedProfile)?;
        let mut selected = vec![account_entry, index_entry];
        for pair in pairs.values() {
            selected.push(pair.header.ok_or(ArchiveError::UnsupportedProfile)?);
            selected.push(pair.messages.ok_or(ArchiveError::UnsupportedProfile)?);
        }
        // This shared streaming validator bounds unknown fields and never retains
        // message values. It also classifies the original numeric ID tokens.
        let report = StructureProbe::inspect_budgeted(
            archive,
            &selected,
            &mut retained_headers,
            &mut header_tokens,
        )
        .map_err(profile_error)?;
        object_shape(&report.entries[0])?;
        object_shape(&report.entries[1])?;
        for (position, _) in pairs.values().enumerate() {
            object_shape(&report.entries[2 + position * 2])?;
            transcript(&report.entries[3 + position * 2])?;
        }
        drop(report);
        let account = read_header(
            archive,
            account_entry,
            &mut retained_headers,
            &mut header_tokens,
        )?;
        let account = AccountHeader {
            id: id_field(&account, "id")?,
            username: string(&account, "username")?,
        };
        let mut contexts = Vec::new();
        for (path_id, pair) in pairs {
            let header_entry = pair.header.ok_or(ArchiveError::UnsupportedProfile)?;
            let messages_entry = pair.messages.ok_or(ArchiveError::UnsupportedProfile)?;
            let value = read_header(
                archive,
                header_entry,
                &mut retained_headers,
                &mut header_tokens,
            )?;
            let id = id_field(&value, "id")?;
            if id != path_id {
                return Err(ArchiveError::InvalidProfile);
            }
            let source_type = string(&value, "type")?;
            let name = match value.get("name") {
                None | Some(Value::Null) => None,
                Some(Value::String(name)) => Some(name.clone()),
                _ => return Err(ArchiveError::InvalidProfile),
            };
            let recipients = value
                .get("recipients")
                .map(|value| {
                    value
                        .as_array()
                        .ok_or(ArchiveError::InvalidProfile)?
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(ArchiveError::InvalidProfile)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            let guild = value
                .get("guild")
                .map(|value| {
                    let value = value.as_object().ok_or(ArchiveError::InvalidProfile)?;
                    Ok(GuildHeader {
                        id: id_field(value, "id")?,
                        name: string(value, "name")?,
                    })
                })
                .transpose()?;
            if guild.is_some() && (recipients.is_some() || name.is_none()) {
                return Err(ArchiveError::InvalidProfile);
            }
            contexts.push(ContextInspection {
                header_entry,
                messages_entry,
                header: ContextHeader {
                    id,
                    source_type,
                    name,
                    recipients,
                    guild,
                },
            });
        }
        Ok(ProfileInspection {
            schema_key: "discord.data_package.messages_json",
            schema_version: 1,
            policy_key: "discord.import_policy.v1",
            account_entry,
            index_entry,
            account,
            contexts,
        })
    }
}

fn profile_error(error: ArchiveError) -> ArchiveError {
    match error {
        ArchiveError::InvalidJson | ArchiveError::DuplicateJsonKey => ArchiveError::InvalidProfile,
        other => other,
    }
}
fn valid_id(value: &str) -> Result<(), ArchiveError> {
    if decimal_grammar(value) == DecimalGrammar::CanonicalPositiveU64Decimal {
        Ok(())
    } else {
        Err(ArchiveError::InvalidProfile)
    }
}
fn string(value: &Map<String, Value>, key: &str) -> Result<String, ArchiveError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(ArchiveError::InvalidProfile)
}
fn id_field(value: &Map<String, Value>, key: &str) -> Result<String, ArchiveError> {
    let value = string(value, key)?;
    valid_id(&value)?;
    Ok(value)
}
fn object_shape(entry: &EntryStructure) -> Result<(), ArchiveError> {
    if matches!(entry.shape, JsonShape::Object(_)) {
        Ok(())
    } else {
        Err(ArchiveError::UnsupportedProfile)
    }
}
fn singleton<T: PartialEq>(set: &GrammarSet<T>, expected: T) -> bool {
    matches!(set, GrammarSet::Observed(values) if values.as_slice() == [expected])
}
fn transcript(entry: &EntryStructure) -> Result<(), ArchiveError> {
    let JsonShape::Array(items) = &entry.shape else {
        return Err(ArchiveError::UnsupportedProfile);
    };
    let item = entry.nodes.iter().find(|node| node.path == ["[]"]);
    if item.is_none() {
        return Ok(());
    } // truly empty array, not heterogeneous evidence
    let JsonShape::Object(fields) = items.as_ref() else {
        return Err(ArchiveError::UnsupportedProfile);
    };
    for (key, expected) in [
        (
            "ID",
            JsonShape::Number {
                integer: true,
                signed: false,
            },
        ),
        ("Timestamp", JsonShape::String),
        ("Contents", JsonShape::String),
        ("Attachments", JsonShape::String),
    ] {
        if fields.get(key) != Some(&expected) {
            return Err(ArchiveError::InvalidProfile);
        }
        let node = entry
            .nodes
            .iter()
            .find(|node| node.path == ["[]", key])
            .ok_or(ArchiveError::InvalidProfile)?;
        if node.missing != 0 {
            return Err(ArchiveError::InvalidProfile);
        }
        if key == "ID"
            && !singleton(
                &node.grammars.decimal_numbers,
                DecimalGrammar::CanonicalPositiveU64Decimal,
            )
        {
            return Err(ArchiveError::InvalidProfile);
        }
        if key == "Timestamp"
            && !singleton(
                &node.grammars.timestamps,
                TimestampGrammar::Calendar {
                    separator: TimestampSeparator::Space,
                    precision: TimestampPrecision::Seconds,
                    zone: TimestampZone::Unzoned,
                },
            )
        {
            return Err(ArchiveError::InvalidProfile);
        }
    }
    Ok(())
}

fn read_header<R: Read + Seek>(
    archive: &mut ArchiveInventory<'_, R>,
    index: EntryIndex,
    retained: &mut u64,
    tokens: &mut u64,
) -> Result<Map<String, Value>, ArchiveError> {
    let limits = archive.limits;
    let cancel = archive.cancel;
    archive
        .consume(index, |reader| {
            // Validate the SAME immutable bytes that will be decoded, not an earlier
            // pass over a potentially mutable Read+Seek implementation. Allocation is
            // bounded before Serde may construct any recursive or scalar values.
            let mut bytes = Vec::new();
            reader
                .take(limits.max_raw_record_bytes + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ArchiveError::ReadFailure)?;
            bounded(bytes.len() as u64, limits.max_raw_record_bytes)?;
            let mut structure_bytes = 0;
            let before_tokens = *tokens;
            let shape = inspect_reader(
                &mut bytes.as_slice(),
                limits,
                cancel,
                &mut structure_bytes,
                tokens,
                Vec::new(),
            )
            .map_err(profile_error)?;
            object_shape(&shape)?;
            // A conservative aggregate ceiling covers typed strings, Vec/Map slots,
            // and allocation growth; ignored fields count too, before Value allocation.
            let typed_slots = (*tokens - before_tokens)
                .checked_mul(size_of::<Value>() as u64 * 2)
                .ok_or(ArchiveError::LimitExceeded)?;
            *retained = add(
                *retained,
                add(add(bytes.len() as u64 * 4, typed_slots)?, structure_bytes)?,
            )?;
            bounded(*retained, limits.max_structure_bytes)?;
            drop(shape);
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| ArchiveError::InvalidProfile)?;
            cancelled(cancel)?;
            match value {
                Value::Object(value) => Ok(value),
                _ => Err(ArchiveError::InvalidProfile),
            }
        })
        .map_err(profile_error)
}
