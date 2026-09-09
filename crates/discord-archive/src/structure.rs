use crate::{
    ArchiveError, ArchiveInventory, ArchiveLimits, Cancellation, EntryIndex,
    inventory::cancelled,
    limits::{add, bounded},
};
use serde::{
    Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{self, BufReader, Read, Seek},
};

/// The only retained JSON value information. No raw scalar payload is representable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum JsonShape {
    Null,
    Boolean,
    Number { integer: bool, signed: bool },
    String,
    Array(Box<JsonShape>),
    Object(BTreeMap<String, JsonShape>),
    Mixed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathToken {
    Account,
    Messages,
    Channels,
    Data,
    Files,
    Index,
    AccountJson,
    MessagesJson,
    ChannelJson,
    IndexJson,
    DataJson,
    DecimalIdentifier,
    MixedIdentifier,
    RedactedSegment,
}

fn template(name: &str) -> Vec<PathToken> {
    name.split('/')
        .map(|part| match part {
            "account" => PathToken::Account,
            "messages" => PathToken::Messages,
            "channels" => PathToken::Channels,
            "data" => PathToken::Data,
            "files" => PathToken::Files,
            "index" => PathToken::Index,
            "account.json" => PathToken::AccountJson,
            "messages.json" => PathToken::MessagesJson,
            "channel.json" => PathToken::ChannelJson,
            "index.json" => PathToken::IndexJson,
            "data.json" => PathToken::DataJson,
            value if value.bytes().all(|b| b.is_ascii_digit()) => PathToken::DecimalIdentifier,
            value if value.bytes().any(|b| b.is_ascii_digit()) => PathToken::MixedIdentifier,
            _ => PathToken::RedactedSegment,
        })
        .collect()
}

#[derive(Debug, Default, Serialize)]
pub struct TypeCounts {
    pub null: u64,
    pub boolean: u64,
    pub unsigned_integer: u64,
    pub signed_integer: u64,
    pub number: u64,
    pub string: u64,
    pub array: u64,
    pub object: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct NodeSummary {
    /// Key names, with `[]` for array items; literal `[]` keys use `~[]`.
    /// A leading `~` in a key is doubled, so paths remain unambiguous.
    pub path: Vec<String>,
    pub occurrences: u64,
    pub missing: u64,
    pub types: TypeCounts,
}

#[derive(Debug, Serialize)]
pub struct EntryStructure {
    pub path: Vec<PathToken>,
    pub shape: JsonShape,
    pub nodes: Vec<NodeSummary>,
    pub max_depth: u64,
}

#[derive(Debug, Serialize)]
pub struct StructureReport {
    pub entries: Vec<EntryStructure>,
}

pub struct StructureProbe;

impl StructureProbe {
    pub fn inspect<R: Read + Seek>(
        archive: &mut ArchiveInventory<'_, R>,
        selected_entries: &[EntryIndex],
    ) -> Result<StructureReport, ArchiveError> {
        let limits = archive.limits;
        let cancel = archive.cancel;
        cancelled(cancel)?;
        bounded(selected_entries.len() as u64, limits.max_selected_contexts)?;
        let mut selected = BTreeSet::new();
        for index in selected_entries {
            if !selected.insert(*index) {
                return Err(ArchiveError::InvalidSelection);
            }
            archive.selected_name(*index)?;
        }
        let mut entries = Vec::new();
        let mut retained = 0;
        let mut tokens = 0;
        for &index in selected_entries {
            cancelled(cancel)?;
            let path = template(archive.selected_name(index)?);
            retained = add(retained, 256 + path.len() as u64 * 16)?;
            bounded(retained, limits.max_structure_bytes)?;
            let entry = archive.consume(index, |reader| {
                let failure = Cell::new(None);
                let guard = LexicalGuard {
                    inner: BufReader::with_capacity(8192, reader),
                    limits,
                    failure: &failure,
                    depth: 0,
                    in_string: false,
                    escaped: false,
                    scalar_bytes: 0,
                    number: false,
                    started: false,
                    root_array: false,
                    record_bytes: 0,
                };
                let mut state = State {
                    limits,
                    cancel,
                    failure: &failure,
                    nodes: BTreeMap::new(),
                    max_depth: 0,
                    decoded: 0,
                    retained: &mut retained,
                    tokens: &mut tokens,
                };
                let mut deserializer = serde_json::Deserializer::from_reader(guard);
                let parsed = ShapeSeed {
                    state: &mut state,
                    path: Vec::new(),
                    depth: 1,
                }
                .deserialize(&mut deserializer);
                let shape =
                    parsed.map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidJson))?;
                deserializer
                    .end()
                    .map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidJson))?;
                let mut missing = Vec::new();
                for (path, node) in &state.nodes {
                    if !path.is_empty() && path.last().is_some_and(|key| key != "[]") {
                        let parent = &path[..path.len() - 1];
                        let count = state.nodes.get(parent).map_or(0, |p| p.types.object);
                        missing.push((path.clone(), count.saturating_sub(node.occurrences)));
                    }
                }
                for (path, count) in missing {
                    state.nodes.get_mut(&path).expect("known node").missing = count;
                }
                Ok(EntryStructure {
                    path,
                    shape,
                    nodes: state.nodes.into_values().collect(),
                    max_depth: state.max_depth,
                })
            })?;
            entries.push(entry);
        }
        Ok(StructureReport { entries })
    }
}

fn merge(target: &mut JsonShape, incoming: JsonShape) {
    match (target, incoming) {
        (JsonShape::Object(left), JsonShape::Object(right)) => {
            for (key, shape) in right {
                if let Some(previous) = left.get_mut(&key) {
                    merge(previous, shape);
                } else {
                    left.insert(key, shape);
                }
            }
        }
        (JsonShape::Array(left), JsonShape::Array(right)) => merge(left, *right),
        (left, right) if *left == right => (),
        (left, _) => *left = JsonShape::Mixed,
    }
}

struct State<'a> {
    limits: ArchiveLimits,
    cancel: &'a dyn Cancellation,
    failure: &'a Cell<Option<ArchiveError>>,
    nodes: BTreeMap<Vec<String>, NodeSummary>,
    max_depth: u64,
    decoded: u64,
    retained: &'a mut u64,
    tokens: &'a mut u64,
}

impl State<'_> {
    fn reject<E: de::Error>(&self, error: ArchiveError) -> E {
        self.failure.set(Some(error));
        E::custom("structure rejected")
    }

    fn check<E: de::Error>(&self, result: Result<(), ArchiveError>) -> Result<(), E> {
        result.map_err(|error| self.reject(error))
    }

    fn scalar<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.check(bounded(bytes as u64, self.limits.max_scalar_bytes))?;
        self.decoded = add(self.decoded, bytes as u64).map_err(|e| self.reject(e))?;
        self.check(bounded(self.decoded, self.limits.max_decoded_record_bytes))
    }

    fn token<E: de::Error>(&mut self) -> Result<(), E> {
        self.check(cancelled(self.cancel))?;
        *self.tokens = add(*self.tokens, 1).map_err(|e| self.reject(e))?;
        self.check(bounded(*self.tokens, self.limits.max_json_tokens))
    }

    fn record<E: de::Error>(&mut self, path: &[String], shape: &JsonShape) -> Result<(), E> {
        if !self.nodes.contains_key(path) {
            // Account for duplicated shape keys, path keys, tree nodes and containers.
            let bytes = path
                .iter()
                .try_fold(256u64, |total, key| add(total, 3 * key.len() as u64 + 96))
                .map_err(|e| self.reject(e))?;
            *self.retained = add(*self.retained, bytes).map_err(|e| self.reject(e))?;
            self.check(bounded(*self.retained, self.limits.max_structure_bytes))?;
            self.nodes.insert(
                path.to_vec(),
                NodeSummary {
                    path: path.to_vec(),
                    ..NodeSummary::default()
                },
            );
        }
        let node = self.nodes.get_mut(path).expect("inserted node");
        node.occurrences += 1;
        let count = match shape {
            JsonShape::Null => &mut node.types.null,
            JsonShape::Boolean => &mut node.types.boolean,
            JsonShape::Number {
                integer: true,
                signed: false,
            } => &mut node.types.unsigned_integer,
            JsonShape::Number {
                integer: true,
                signed: true,
            } => &mut node.types.signed_integer,
            JsonShape::Number { integer: false, .. } => &mut node.types.number,
            JsonShape::String => &mut node.types.string,
            JsonShape::Array(_) => &mut node.types.array,
            JsonShape::Object(_) => &mut node.types.object,
            JsonShape::Mixed => return Ok(()),
        };
        *count += 1;
        Ok(())
    }
}

struct ShapeSeed<'a, 'b> {
    state: &'a mut State<'b>,
    path: Vec<String>,
    depth: u64,
}

impl<'de> DeserializeSeed<'de> for ShapeSeed<'_, '_> {
    type Value = JsonShape;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<JsonShape, D::Error> {
        self.state.token()?;
        self.state
            .check(bounded(self.depth, self.state.limits.max_json_depth))?;
        self.state.max_depth = self.state.max_depth.max(self.depth);
        let path = self.path.clone();
        let state = self.state;
        let shape = deserializer.deserialize_any(ShapeVisitor {
            state,
            path: self.path,
            depth: self.depth,
        })?;
        state.record(&path, &shape)?;
        Ok(shape)
    }
}

struct ShapeVisitor<'a, 'b> {
    state: &'a mut State<'b>,
    path: Vec<String>,
    depth: u64,
}

impl<'de> Visitor<'de> for ShapeVisitor<'_, '_> {
    type Value = JsonShape;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_unit<E: de::Error>(self) -> Result<JsonShape, E> {
        self.state.scalar(1)?;
        Ok(JsonShape::Null)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<JsonShape, E> {
        self.state.scalar(1)?;
        Ok(JsonShape::Boolean)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<JsonShape, E> {
        self.state.scalar(size_of::<u64>())?;
        Ok(JsonShape::Number {
            integer: true,
            signed: false,
        })
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<JsonShape, E> {
        self.state.scalar(size_of::<i64>())?;
        Ok(JsonShape::Number {
            integer: true,
            signed: true,
        })
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<JsonShape, E> {
        self.state.scalar(size_of::<f64>())?;
        Ok(JsonShape::Number {
            integer: false,
            signed: value.is_sign_negative(),
        })
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<JsonShape, E> {
        self.state.scalar(value.len())?;
        Ok(JsonShape::String)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<JsonShape, A::Error> {
        let mut shape = None;
        let mut path = self.path;
        path.push("[]".into());
        loop {
            if self.depth == 1 {
                self.state.decoded = 0;
            }
            let next = sequence.next_element_seed(ShapeSeed {
                state: self.state,
                path: path.clone(),
                depth: self.depth + 1,
            })?;
            let Some(next) = next else { break };
            match &mut shape {
                Some(previous) => merge(previous, next),
                None => shape = Some(next),
            }
        }
        Ok(JsonShape::Array(Box::new(
            shape.unwrap_or(JsonShape::Mixed),
        )))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsonShape, A::Error> {
        let mut fields = BTreeMap::new();
        while let Some(key) = map.next_key_seed(KeySeed(self.state))? {
            if fields.contains_key(&key) {
                return Err(self.state.reject(ArchiveError::DuplicateJsonKey));
            }
            let mut path = self.path.clone();
            path.push(if key == "[]" || key.starts_with('~') {
                format!("~{key}")
            } else {
                key.clone()
            });
            let shape = map.next_value_seed(ShapeSeed {
                state: self.state,
                path,
                depth: self.depth + 1,
            })?;
            fields.insert(key, shape);
        }
        Ok(JsonShape::Object(fields))
    }
}

struct KeySeed<'a, 'b>(&'a mut State<'b>);
impl<'de> DeserializeSeed<'de> for KeySeed<'_, '_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        self.0.token()?;
        deserializer.deserialize_str(self)
    }
}
impl Visitor<'_> for KeySeed<'_, '_> {
    type Value = String;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        self.0.scalar(value.len())?;
        Ok(value.to_owned())
    }
}

/// Bounds the parser's scratch allocation before Serde can allocate a full token.
/// Six raw bytes may encode one decoded byte (`\u0061`); visitors enforce decoded limits.
struct LexicalGuard<'a, R> {
    inner: R,
    limits: ArchiveLimits,
    failure: &'a Cell<Option<ArchiveError>>,
    depth: u64,
    in_string: bool,
    escaped: bool,
    scalar_bytes: u64,
    number: bool,
    started: bool,
    root_array: bool,
    record_bytes: u64,
}

impl<R: Read> LexicalGuard<'_, R> {
    fn observe(&mut self, byte: u8) -> Result<(), ArchiveError> {
        if !self.started && !byte.is_ascii_whitespace() {
            self.started = true;
            self.root_array = byte == b'[';
        }
        let root_separator =
            self.root_array && self.depth == 1 && !self.in_string && matches!(byte, b',' | b']');
        if root_separator {
            self.record_bytes = 0;
        } else if self.started && !(self.root_array && self.depth == 0) {
            self.record_bytes = add(self.record_bytes, 1)?;
            bounded(self.record_bytes, self.limits.max_raw_record_bytes)?;
        }
        if self.in_string {
            if !self.escaped && byte == b'"' {
                self.in_string = false;
                self.scalar_bytes = 0;
            } else {
                self.scalar_bytes = add(self.scalar_bytes, 1)?;
                bounded(self.scalar_bytes, self.limits.max_scalar_bytes * 6)?;
                self.escaped = !self.escaped && byte == b'\\';
            }
        } else {
            match byte {
                b'"' => {
                    self.in_string = true;
                    self.escaped = false;
                    self.scalar_bytes = 0;
                    self.number = false;
                }
                b'[' | b'{' => {
                    self.depth = add(self.depth, 1)?;
                    bounded(self.depth, self.limits.max_json_depth)?;
                    self.number = false;
                }
                b']' | b'}' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.number = false;
                }
                b',' | b':' | b' ' | b'\n' | b'\r' | b'\t' => {
                    self.number = false;
                    self.scalar_bytes = 0;
                }
                _ => {
                    if !self.number {
                        self.scalar_bytes = 0;
                        self.number = true;
                    }
                    self.scalar_bytes = add(self.scalar_bytes, 1)?;
                    bounded(self.scalar_bytes, self.limits.max_scalar_bytes)?;
                }
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for LexicalGuard<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = self.inner.read(&mut bytes[..1])?;
        if count != 0 {
            self.observe(bytes[0]).map_err(|error| {
                self.failure.set(Some(error));
                io::Error::other(error)
            })?;
        }
        Ok(count)
    }
}
