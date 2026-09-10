//! One exact, structural profile. No message decoding or timezone conversion.
use crate::{
    ArchiveError, ArchiveInventory, ArchiveLimits, Cancellation, DecimalGrammar, EntryIndex,
    GrammarSet, JsonShape, StructureProbe, TimestampGrammar, TimestampPrecision,
    TimestampSeparator, TimestampZone,
    inventory::cancelled,
    limits::{add, bounded},
    structure::{EntryStructure, decimal_grammar, inspect_reader},
};
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use std::{
    cell::Cell,
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
            HeaderKind::Account,
        )?
        .account()?;
        let mut contexts = Vec::new();
        for (path_id, pair) in pairs {
            let header_entry = pair.header.ok_or(ArchiveError::UnsupportedProfile)?;
            let messages_entry = pair.messages.ok_or(ArchiveError::UnsupportedProfile)?;
            let header = read_header(
                archive,
                header_entry,
                &mut retained_headers,
                &mut header_tokens,
                HeaderKind::Context,
            )?
            .context()?;
            if header.id != path_id {
                return Err(ArchiveError::InvalidProfile);
            }
            contexts.push(ContextInspection {
                header_entry,
                messages_entry,
                header,
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
    kind: HeaderKind,
) -> Result<HeaderFields, ArchiveError> {
    let limits = archive.limits;
    let cancel = archive.cancel;
    archive
        .consume(index, |reader| {
            // Validate the SAME immutable bytes that will be decoded, not an earlier
            // pass over a potentially mutable Read+Seek implementation. Allocation is
            // bounded before Serde may construct any recursive or scalar values.
            let mut bytes = Vec::new();
            let mut chunk = [0; 256];
            loop {
                cancelled(cancel)?;
                let count = reader
                    .read(&mut chunk)
                    .map_err(|_| ArchiveError::ReadFailure)?;
                if count == 0 {
                    break;
                }
                let length = add(bytes.len() as u64, count as u64)?;
                bounded(length, limits.max_raw_record_bytes)?;
                if length > bytes.capacity() as u64 {
                    let capacity = length
                        .checked_next_power_of_two()
                        .ok_or(ArchiveError::LimitExceeded)?;
                    // Charge the whole new allocation before reserve, not merely
                    // the data length afterward. Prior buffers/headers are never
                    // refunded; even transient reallocation peaks are covered.
                    charge_retained(retained, capacity, limits.max_structure_bytes)?;
                    bytes
                        .try_reserve_exact(capacity as usize - bytes.len())
                        .map_err(|_| ArchiveError::LimitExceeded)?;
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            let before_tokens = *tokens;
            let shape = inspect_reader(
                &mut bytes.as_slice(),
                limits,
                cancel,
                retained,
                tokens,
                Vec::new(),
            )
            .map_err(profile_error)?;
            object_shape(&shape)?;
            drop(shape);
            // Reserve the validated token count for the field-selective pass too.
            // IgnoredAny therefore cannot create a fresh parsing-work allowance.
            let decoding_tokens = *tokens - before_tokens;
            *tokens = add(*tokens, decoding_tokens)?;
            bounded(*tokens, limits.max_json_tokens)?;
            let failure = Cell::new(None);
            let mut state = HeaderState {
                limits,
                cancel,
                retained,
                failure: &failure,
            };
            let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
            let header = HeaderSeed {
                state: &mut state,
                kind,
            }
            .deserialize(&mut deserializer)
            .map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidProfile))?;
            deserializer
                .end()
                .map_err(|_| ArchiveError::InvalidProfile)?;
            cancelled(cancel)?;
            Ok(header)
        })
        .map_err(profile_error)
}

fn charge_retained(retained: &mut u64, bytes: u64, maximum: u64) -> Result<(), ArchiveError> {
    let next = add(*retained, bytes)?;
    bounded(next, maximum)?;
    *retained = next;
    Ok(())
}

#[derive(Clone, Copy)]
enum HeaderKind {
    Account,
    Context,
    Guild,
}
#[derive(Default)]
struct HeaderFields {
    id: Option<String>,
    username: Option<String>,
    source_type: Option<String>,
    name: Option<String>,
    recipients: Option<Vec<String>>,
    guild: Option<GuildHeader>,
}
fn required<T>(value: Option<T>) -> Result<T, ArchiveError> {
    value.ok_or(ArchiveError::InvalidProfile)
}
impl HeaderFields {
    fn account(self) -> Result<AccountHeader, ArchiveError> {
        Ok(AccountHeader {
            id: required(self.id)?,
            username: required(self.username)?,
        })
    }
    fn guild(self) -> Result<GuildHeader, ArchiveError> {
        Ok(GuildHeader {
            id: required(self.id)?,
            name: required(self.name)?,
        })
    }
    fn context(self) -> Result<ContextHeader, ArchiveError> {
        if self.guild.is_some() && (self.recipients.is_some() || self.name.is_none()) {
            return Err(ArchiveError::InvalidProfile);
        }
        Ok(ContextHeader {
            id: required(self.id)?,
            source_type: required(self.source_type)?,
            name: self.name,
            recipients: self.recipients,
            guild: self.guild,
        })
    }
}
struct HeaderState<'a> {
    limits: ArchiveLimits,
    cancel: &'a dyn Cancellation,
    retained: &'a mut u64,
    failure: &'a Cell<Option<ArchiveError>>,
}
impl HeaderState<'_> {
    fn check<E: de::Error>(&self, result: Result<(), ArchiveError>) -> Result<(), E> {
        result.map_err(|error| {
            self.failure.set(Some(error));
            E::custom("header rejected")
        })
    }
    fn charge<E: de::Error>(&mut self, bytes: u64) -> Result<(), E> {
        self.check(cancelled(self.cancel))?;
        let charged = charge_retained(self.retained, bytes, self.limits.max_structure_bytes);
        self.check(charged)
    }
}

#[derive(Clone, Copy)]
enum HeaderKey {
    Id,
    Username,
    Type,
    Name,
    Recipients,
    Guild,
    Other,
}
impl<'de> Deserialize<'de> for HeaderKey {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;
        impl Visitor<'_> for KeyVisitor {
            type Value = HeaderKey;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("header key")
            }
            fn visit_str<E: de::Error>(self, key: &str) -> Result<HeaderKey, E> {
                Ok(match key {
                    "id" => HeaderKey::Id,
                    "username" => HeaderKey::Username,
                    "type" => HeaderKey::Type,
                    "name" => HeaderKey::Name,
                    "recipients" => HeaderKey::Recipients,
                    "guild" => HeaderKey::Guild,
                    _ => HeaderKey::Other,
                })
            }
        }
        deserializer.deserialize_str(KeyVisitor)
    }
}
struct HeaderSeed<'a, 'b> {
    state: &'a mut HeaderState<'b>,
    kind: HeaderKind,
}
impl<'de> DeserializeSeed<'de> for HeaderSeed<'_, '_> {
    type Value = HeaderFields;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<HeaderFields, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for HeaderSeed<'_, '_> {
    type Value = HeaderFields;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("header object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HeaderFields, A::Error> {
        let mut fields = HeaderFields::default();
        let mut seen = 0u8;
        while let Some(key) = map.next_key::<HeaderKey>()? {
            self.state.check(cancelled(self.state.cancel))?;
            let accepted = matches!(
                (self.kind, key),
                (_, HeaderKey::Id)
                    | (HeaderKind::Account, HeaderKey::Username)
                    | (
                        HeaderKind::Context,
                        HeaderKey::Type
                            | HeaderKey::Name
                            | HeaderKey::Recipients
                            | HeaderKey::Guild
                    )
                    | (HeaderKind::Guild, HeaderKey::Name)
            );
            if !accepted {
                // The exact immutable buffer was already guarded for all limits,
                // including duplicate keys and ignored nested scalar/container data.
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let bit = 1 << key as u8;
            if seen & bit != 0 {
                return Err(de::Error::custom("duplicate header field"));
            }
            seen |= bit;
            match key {
                HeaderKey::Id => {
                    fields.id = Some(map.next_value_seed(TextSeed {
                        state: self.state,
                        identifier: true,
                    })?)
                }
                HeaderKey::Username => {
                    fields.username = Some(map.next_value_seed(TextSeed {
                        state: self.state,
                        identifier: false,
                    })?)
                }
                HeaderKey::Type => {
                    fields.source_type = Some(map.next_value_seed(TextSeed {
                        state: self.state,
                        identifier: false,
                    })?)
                }
                HeaderKey::Name => {
                    fields.name = if matches!(self.kind, HeaderKind::Context) {
                        map.next_value_seed(OptionalText(self.state))?
                    } else {
                        Some(map.next_value_seed(TextSeed {
                            state: self.state,
                            identifier: false,
                        })?)
                    }
                }
                HeaderKey::Recipients => {
                    fields.recipients = Some(map.next_value_seed(RecipientsSeed(self.state))?)
                }
                HeaderKey::Guild => {
                    fields.guild = Some(
                        map.next_value_seed(HeaderSeed {
                            state: self.state,
                            kind: HeaderKind::Guild,
                        })?
                        .guild()
                        .map_err(|_| de::Error::custom("invalid guild header"))?,
                    )
                }
                HeaderKey::Other => unreachable!(),
            }
        }
        Ok(fields)
    }
}
struct TextSeed<'a, 'b> {
    state: &'a mut HeaderState<'b>,
    identifier: bool,
}
impl<'de> DeserializeSeed<'de> for TextSeed<'_, '_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_str(self)
    }
}
impl Visitor<'_> for TextSeed<'_, '_> {
    type Value = String;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded header string")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        self.state.check(bounded(
            value.len() as u64,
            self.state.limits.max_scalar_bytes,
        ))?;
        if self.identifier {
            self.state.check(valid_id(value))?;
        } else {
            self.state.check(bounded(
                value.len() as u64,
                self.state.limits.max_display_bytes,
            ))?;
        }
        self.state.charge(value.len() as u64 * 2 + 64)?;
        Ok(value.to_owned())
    }
}
struct OptionalText<'a, 'b>(&'a mut HeaderState<'b>);
impl<'de> DeserializeSeed<'de> for OptionalText<'_, '_> {
    type Value = Option<String>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_option(self)
    }
}
impl<'de> Visitor<'de> for OptionalText<'_, '_> {
    type Value = Option<String>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("optional header string")
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_some<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        TextSeed {
            state: self.0,
            identifier: false,
        }
        .deserialize(deserializer)
        .map(Some)
    }
}
struct RecipientsSeed<'a, 'b>(&'a mut HeaderState<'b>);
impl<'de> DeserializeSeed<'de> for RecipientsSeed<'_, '_> {
    type Value = Vec<String>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for RecipientsSeed<'_, '_> {
    type Value = Vec<String>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("recipient strings")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut recipients = Vec::new();
        while let Some(value) = seq.next_element_seed(TextSeed {
            state: self.0,
            identifier: false,
        })? {
            if recipients.len() == recipients.capacity() {
                let capacity = recipients.len().max(2) * 2;
                self.0
                    .charge(capacity as u64 * size_of::<String>() as u64)?;
                self.0.check(
                    recipients
                        .try_reserve_exact(capacity - recipients.len())
                        .map_err(|_| ArchiveError::LimitExceeded),
                )?;
            }
            recipients.push(value);
        }
        Ok(recipients)
    }
}
