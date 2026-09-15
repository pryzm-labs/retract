//! One-record-at-a-time typed streaming, with no transcript or raw-record tree.
use crate::{
    AccountHeader, ArchiveError, ArchiveInventory, ArchiveLimits, Cancellation, ChannelContext,
    DecimalGrammar, DiscordId, EntryIndex, EntryIntegrity, ExportAccount, GuildContext, JsonShape,
    ProfileInspection, ReadSummary, RecordSink, SentMessage,
    inventory::{CancelRead, EitherCancellation, cancelled},
    limits::{add, bounded},
    profile::{read_account_header, read_context_header},
    record::timestamp_millis,
    structure::{LexicalGuard, inspect_reader},
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::{
    cell::Cell,
    collections::BTreeSet,
    fmt,
    io::{Read, Seek},
};

struct Selection {
    header: EntryIndex,
    messages: EntryIndex,
    id: DiscordId,
}
pub struct DiscordArchiveReader<'a, R> {
    archive: ArchiveInventory<'a, R>,
    cancel: &'a dyn Cancellation,
    account_entry: EntryIndex,
    expected_account: AccountHeader,
    contexts: Vec<Selection>,
    retained: u64,
    tokens: u64,
    attempted: bool,
}

impl<'a, R: Read + Seek> DiscordArchiveReader<'a, R> {
    pub fn open(
        mut archive: ArchiveInventory<'a, R>,
        mut profile: ProfileInspection,
        cancel: &'a dyn Cancellation,
    ) -> Result<Self, ArchiveError> {
        cancelled(cancel)?;
        archive.bind_reader_cancellation(cancel)?;
        if profile.schema_key != "discord.data_package.messages_json"
            || profile.schema_version != 1
            || profile.policy_key != "discord.import_policy.v1"
            || archive.selected_name(profile.account_entry)? != "Account/user.json"
            || archive.selected_name(profile.index_entry)? != "Messages/index.json"
        {
            return Err(ArchiveError::InvalidProfile);
        }
        let limits = archive.limits;
        bounded(
            add(
                2,
                (profile.contexts.len() as u64)
                    .checked_mul(2)
                    .ok_or(ArchiveError::LimitExceeded)?,
            )?,
            limits.max_selected_contexts,
        )?;
        let mut retained = add(profile.retained_bytes, 256)?;
        bounded(retained, limits.max_structure_bytes)?;
        let mut selected = BTreeSet::from([profile.account_entry, profile.index_entry]);
        // Token work starts a distinct typed parsing pass; retained allocation
        // usage does not reset while inspection headers still coexist with us.
        let mut tokens = 0;
        let account = read_account_header(
            &mut archive,
            profile.account_entry,
            &mut retained,
            &mut tokens,
            cancel,
        )?;
        if account != profile.account {
            return Err(ArchiveError::InvalidProfile);
        }
        let both = EitherCancellation(archive.cancel, cancel);
        let shape = archive.consume(profile.index_entry, |reader| {
            let failure = Cell::new(None);
            let mut reader = CancelRead::new(reader, &both, &failure);
            inspect_reader(
                &mut reader,
                limits,
                &both,
                &mut retained,
                &mut tokens,
                Vec::new(),
            )
            .map_err(|error| failure.get().unwrap_or(error))
        })?;
        if !matches!(shape.shape, JsonShape::Object(_)) {
            return Err(ArchiveError::InvalidProfile);
        }
        drop(shape);
        profile
            .contexts
            .sort_unstable_by(|a, b| a.header.id.cmp(&b.header.id));
        let mut contexts = Vec::new();
        for context in profile.contexts {
            cancelled(cancel)?;
            // Charge IDs, path comparisons, selection-tree nodes and geometric
            // selection capacity before allocation, cumulatively across entries.
            retained = add(retained, 512)?;
            bounded(retained, limits.max_structure_bytes)?;
            let id = DiscordId::parse(&context.header.id)?;
            for (entry, suffix) in [
                (context.header_entry, "channel.json"),
                (context.messages_entry, "messages.json"),
            ] {
                if !selected.insert(entry)
                    || archive.selected_name(entry)?
                        != format!("Messages/c{}/{suffix}", id.as_str())
                {
                    return Err(ArchiveError::InvalidProfile);
                }
            }
            let header = read_context_header(
                &mut archive,
                context.header_entry,
                &mut retained,
                &mut tokens,
                cancel,
            )?;
            if header != context.header {
                return Err(ArchiveError::InvalidProfile);
            }
            contexts.push(Selection {
                header: context.header_entry,
                messages: context.messages_entry,
                id,
            });
        }
        // Public inspections are hints, never authority to omit reserved entries.
        for n in 0..archive.entry_count() {
            cancelled(cancel)?;
            let entry = EntryIndex(n);
            let Ok(name) = archive.selected_name(entry) else {
                continue;
            };
            let file = name.rsplit('/').next().unwrap_or_default();
            if !selected.contains(&entry)
                && ([
                    "user.json",
                    "account.json",
                    "channel.json",
                    "messages.json",
                    "messages.csv",
                ]
                .iter()
                .any(|s| file.eq_ignore_ascii_case(s))
                    || (file.eq_ignore_ascii_case("index.json")
                        && name.split('/').any(|s| s.eq_ignore_ascii_case("Messages"))))
            {
                return Err(ArchiveError::UnsupportedProfile);
            }
        }
        cancelled(cancel)?;
        Ok(Self {
            archive,
            cancel,
            account_entry: profile.account_entry,
            expected_account: account,
            contexts,
            retained,
            tokens,
            attempted: false,
        })
    }

    pub fn read_account(&mut self) -> Result<ExportAccount, ArchiveError> {
        cancelled(self.cancel)?;
        let account = read_account_header(
            &mut self.archive,
            self.account_entry,
            &mut self.retained,
            &mut self.tokens,
            self.cancel,
        )?;
        if account != self.expected_account {
            return Err(ArchiveError::InvalidProfile);
        }
        cancelled(self.cancel)?;
        Ok(ExportAccount {
            id: DiscordId::parse(&account.id)?,
            username: account.username,
        })
    }

    /// A pass is single-use, including failure. Explicit retry creates a fresh
    /// reader/inventory, ensuring failed provisional output cannot be resumed.
    pub fn visit_channels(
        &mut self,
        sink: &mut dyn RecordSink,
    ) -> Result<ReadSummary, ArchiveError> {
        cancelled(self.cancel)?;
        if self.attempted {
            return Err(ArchiveError::InvalidSelection);
        }
        self.attempted = true;
        let account = self.read_account()?;
        let limits = self.archive.limits;
        let mut summary = ReadSummary::default();
        for selection in &self.contexts {
            cancelled(self.cancel)?;
            let header = read_context_header(
                &mut self.archive,
                selection.header,
                &mut self.retained,
                &mut self.tokens,
                self.cancel,
            )?;
            let id = DiscordId::parse(&header.id)?;
            if id != selection.id {
                return Err(ArchiveError::InvalidProfile);
            }
            let channel = ChannelContext {
                id: id.clone(),
                source_type: header.source_type,
                name: header.name,
                recipients: header.recipients,
                guild: header
                    .guild
                    .map(|guild| {
                        Ok(GuildContext {
                            id: DiscordId::parse(&guild.id)?,
                            name: guild.name,
                        })
                    })
                    .transpose()?,
            };
            cancelled(self.cancel)?;
            sink.begin_channel(channel)?;
            let both = EitherCancellation(self.archive.cancel, self.cancel);
            let count = self.archive.consume(selection.messages, |reader| {
                stream(
                    reader,
                    limits,
                    &both,
                    &mut self.tokens,
                    &account.id,
                    &id,
                    sink,
                )
            })?;
            cancelled(self.cancel)?;
            sink.end_channel(EntryIntegrity {
                entry: selection.messages,
                messages: count,
            })?;
            summary.channels = add(summary.channels, 1)?;
            summary.messages = add(summary.messages, count)?;
        }
        cancelled(self.cancel)?;
        Ok(summary)
    }
}

struct State<'a> {
    limits: ArchiveLimits,
    cancel: &'a dyn Cancellation,
    failure: &'a Cell<Option<ArchiveError>>,
    numeric: &'a Cell<DecimalGrammar>,
    tokens: &'a mut u64,
    decoded: u64,
}
impl State<'_> {
    fn reject<E: de::Error>(&self, error: ArchiveError) -> E {
        self.failure.set(Some(error));
        E::custom("record rejected")
    }
    fn check<T, E: de::Error>(&self, result: Result<T, ArchiveError>) -> Result<T, E> {
        result.map_err(|e| self.reject(e))
    }
    fn token<E: de::Error>(&mut self) -> Result<(), E> {
        self.check(cancelled(self.cancel))?;
        *self.tokens = self.check(add(*self.tokens, 1))?;
        self.check(bounded(*self.tokens, self.limits.max_json_tokens))
    }
    fn scalar<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.check(bounded(bytes as u64, self.limits.max_scalar_bytes))?;
        self.decoded = self.check(add(self.decoded, bytes as u64))?;
        self.check(bounded(self.decoded, self.limits.max_decoded_record_bytes))
    }
}
fn stream(
    reader: &mut dyn Read,
    limits: ArchiveLimits,
    cancel: &dyn Cancellation,
    tokens: &mut u64,
    account: &DiscordId,
    channel: &DiscordId,
    sink: &mut dyn RecordSink,
) -> Result<u64, ArchiveError> {
    let failure = Cell::new(None);
    let numeric = Cell::new(DecimalGrammar::Other);
    let guard = LexicalGuard::new(
        CancelRead::new(reader, cancel, &failure),
        limits,
        &failure,
        &numeric,
    );
    let mut de = serde_json::Deserializer::from_reader(guard);
    let mut state = State {
        limits,
        cancel,
        failure: &failure,
        numeric: &numeric,
        tokens,
        decoded: 0,
    };
    let count = Transcript {
        state: &mut state,
        account,
        channel,
        sink,
    }
    .deserialize(&mut de)
    .map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidJson))?;
    de.end()
        .map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidJson))?;
    cancelled(cancel)?;
    Ok(count)
}

struct Transcript<'a, 'b> {
    state: &'a mut State<'b>,
    account: &'a DiscordId,
    channel: &'a DiscordId,
    sink: &'a mut dyn RecordSink,
}
impl<'de> DeserializeSeed<'de> for Transcript<'_, '_> {
    type Value = u64;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<u64, D::Error> {
        self.state.token()?;
        de.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Transcript<'_, '_> {
    type Value = u64;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("message array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u64, A::Error> {
        let mut count = 0;
        loop {
            self.state.decoded = 0;
            self.state.check(cancelled(self.state.cancel))?;
            let Some(record) = seq.next_element_seed(Message {
                state: self.state,
                account: self.account,
                channel: self.channel,
            })?
            else {
                break;
            };
            self.state.check(cancelled(self.state.cancel))?;
            self.state.check(self.sink.message(record))?;
            count = self.state.check(add(count, 1))?;
        }
        Ok(count)
    }
}
#[derive(Clone, Copy)]
enum Key {
    Id,
    Timestamp,
    Contents,
    Attachments,
    Author,
    Other,
}
struct KeySeed<'a, 'b>(&'a mut State<'b>);
impl<'de> DeserializeSeed<'de> for KeySeed<'_, '_> {
    type Value = Key;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<Key, D::Error> {
        self.0.token()?;
        de.deserialize_str(self)
    }
}
impl Visitor<'_> for KeySeed<'_, '_> {
    type Value = Key;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("record key")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Key, E> {
        self.0.scalar(s.len())?;
        Ok(match s {
            "ID" => Key::Id,
            "Timestamp" => Key::Timestamp,
            "Contents" => Key::Contents,
            "Attachments" => Key::Attachments,
            "author" | "Author" | "author_id" | "authorId" | "AuthorID" => Key::Author,
            _ => Key::Other,
        })
    }
}
struct Message<'a, 'b> {
    state: &'a mut State<'b>,
    account: &'a DiscordId,
    channel: &'a DiscordId,
}
impl<'de> DeserializeSeed<'de> for Message<'_, '_> {
    type Value = SentMessage;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<SentMessage, D::Error> {
        self.state.token()?;
        de.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Message<'_, '_> {
    type Value = SentMessage;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("message object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<SentMessage, A::Error> {
        let (mut id, mut timestamp, mut contents, mut attachments) = (None, None, None, None);
        let mut seen = 0u8;
        while let Some(key) = map.next_key_seed(KeySeed(self.state))? {
            if matches!(key, Key::Other) {
                map.next_value_seed(Skip(self.state))?;
                continue;
            }
            // The frozen sent-message profile has no author field. Any explicit
            // author variant needs separate evidence; it cannot override account.
            if matches!(key, Key::Author) {
                return Err(self.state.reject(ArchiveError::InvalidProfile));
            }
            let bit = 1 << key as u8;
            if seen & bit != 0 {
                return Err(self.state.reject(ArchiveError::DuplicateJsonKey));
            }
            seen |= bit;
            match key {
                Key::Id => id = Some(map.next_value_seed(IdSeed(self.state))?),
                Key::Timestamp => timestamp = Some(map.next_value_seed(Text(self.state))?),
                Key::Contents => contents = Some(map.next_value_seed(Text(self.state))?),
                Key::Attachments => attachments = Some(map.next_value_seed(Text(self.state))?),
                _ => unreachable!(),
            }
        }
        let id = id.ok_or_else(|| self.state.reject(ArchiveError::InvalidProfile))?;
        let timestamp = timestamp.ok_or_else(|| self.state.reject(ArchiveError::InvalidProfile))?;
        Ok(SentMessage {
            timestamp_millis: self.state.check(timestamp_millis(&id, &timestamp))?,
            id,
            account_id: self.account.clone(),
            channel_id: self.channel.clone(),
            contents: contents.ok_or_else(|| self.state.reject(ArchiveError::InvalidProfile))?,
            attachments: attachments
                .ok_or_else(|| self.state.reject(ArchiveError::InvalidProfile))?,
        })
    }
}
struct IdSeed<'a, 'b>(&'a mut State<'b>);
impl<'de> DeserializeSeed<'de> for IdSeed<'_, '_> {
    type Value = DiscordId;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<DiscordId, D::Error> {
        self.0.token()?;
        de.deserialize_u64(self)
    }
}
impl Visitor<'_> for IdSeed<'_, '_> {
    type Value = DiscordId;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("canonical positive integer")
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<DiscordId, E> {
        self.0.scalar(size_of::<u64>())?;
        if self.0.numeric.get() != DecimalGrammar::CanonicalPositiveU64Decimal {
            return Err(self.0.reject(ArchiveError::InvalidProfile));
        }
        self.0.check(DiscordId::parse(&value.to_string()))
    }
}
struct Text<'a, 'b>(&'a mut State<'b>);
impl<'de> DeserializeSeed<'de> for Text<'_, '_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<String, D::Error> {
        self.0.token()?;
        de.deserialize_str(self)
    }
}
impl Visitor<'_> for Text<'_, '_> {
    type Value = String;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded string")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        self.0.scalar(value.len())?;
        Ok(value.to_owned())
    }
}
// Every ignored value/key consumes the same scalar/decoded/token/depth limits.
// Nothing here allocates a generic Value, map, array or retained unknown string.
struct Skip<'a, 'b>(&'a mut State<'b>);
impl<'de> DeserializeSeed<'de> for Skip<'_, '_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        self.0.token()?;
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Skip<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded ignored value")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.0.scalar(1)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.0.scalar(1)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        self.0.scalar(8)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.0.scalar(8)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.0.scalar(8)
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<(), E> {
        self.0.scalar(s.len())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(Skip(self.0))?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key_seed(KeySeed(self.0))?.is_some() {
            map.next_value_seed(Skip(self.0))?;
        }
        Ok(())
    }
}
