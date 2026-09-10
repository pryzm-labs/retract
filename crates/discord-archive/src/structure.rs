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

/// Keep absence of array-item evidence distinct from observed heterogeneous items.
/// Only the completed report maps an unobserved item shape to public `Mixed`.
#[derive(Eq, PartialEq)]
enum ObservedShape {
    Null,
    Boolean,
    Number { integer: bool, signed: bool },
    String,
    Array(Option<Box<ObservedShape>>),
    Object(BTreeMap<String, ObservedShape>),
    Mixed,
}

impl ObservedShape {
    fn into_report(self) -> JsonShape {
        match self {
            Self::Null => JsonShape::Null,
            Self::Boolean => JsonShape::Boolean,
            Self::Number { integer, signed } => JsonShape::Number { integer, signed },
            Self::String => JsonShape::String,
            Self::Array(items) => JsonShape::Array(Box::new(
                items.map_or(JsonShape::Mixed, |items| items.into_report()),
            )),
            Self::Object(fields) => JsonShape::Object(
                fields
                    .into_iter()
                    .map(|(key, shape)| (key, shape.into_report()))
                    .collect(),
            ),
            Self::Mixed => JsonShape::Mixed,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathToken {
    Account,
    Messages,
    TitleCaseAccount,
    TitleCaseMessages,
    Channels,
    Data,
    Files,
    Index,
    AccountJson,
    UserJson,
    MessagesJson,
    ChannelJson,
    IndexJson,
    DataJson,
    DecimalIdentifier,
    LowercaseCPrefixedCanonicalPositiveU64Decimal,
    MixedIdentifier,
    RedactedSegment,
}

fn template(name: &str) -> Vec<PathToken> {
    name.split('/')
        .map(|part| match part {
            "account" => PathToken::Account,
            "messages" => PathToken::Messages,
            "Account" => PathToken::TitleCaseAccount,
            "Messages" => PathToken::TitleCaseMessages,
            "channels" => PathToken::Channels,
            "data" => PathToken::Data,
            "files" => PathToken::Files,
            "index" => PathToken::Index,
            "account.json" => PathToken::AccountJson,
            "user.json" => PathToken::UserJson,
            "messages.json" => PathToken::MessagesJson,
            "channel.json" => PathToken::ChannelJson,
            "index.json" => PathToken::IndexJson,
            "data.json" => PathToken::DataJson,
            value if decimal_grammar(value) == DecimalGrammar::CanonicalPositiveU64Decimal => {
                PathToken::DecimalIdentifier
            }
            value
                if value.strip_prefix('c').is_some_and(|rest| {
                    decimal_grammar(rest) == DecimalGrammar::CanonicalPositiveU64Decimal
                }) =>
            {
                PathToken::LowercaseCPrefixedCanonicalPositiveU64Decimal
            }
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

/// Closed syntax categories, not identifiers or numeric values.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum DecimalGrammar {
    CanonicalPositiveU64Decimal,
    Zero,
    NoncanonicalDecimal,
    OutOfU64Range,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TimestampSeparator {
    UpperT,
    Space,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TimestampPrecision {
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TimestampZone {
    UpperZ,
    ColonOffset,
    Unzoned,
}

/// Gregorian calendar years 0001..9999, with no leap seconds or inferred zone.
/// The finite product has 24 calendar alternatives and one unknown alternative.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TimestampGrammar {
    Unclassified,
    Calendar {
        separator: TimestampSeparator,
        precision: TimestampPrecision,
        zone: TimestampZone,
    },
}

/// An unobserved scalar kind must not look like observed, unclassified input.
#[derive(Debug, Default, Serialize)]
pub enum GrammarSet<T> {
    #[default]
    NoObservation,
    /// Sorted and deduplicated by the probe; contains only closed enum members.
    Observed(Vec<T>),
}

impl<T: Ord> GrammarSet<T> {
    fn observe(
        &mut self,
        grammar: T,
        retained: &mut u64,
        maximum: u64,
    ) -> Result<(), ArchiveError> {
        let position = match self {
            Self::NoObservation => 0,
            Self::Observed(grammars) => match grammars.binary_search(&grammar) {
                Ok(_) => return Ok(()),
                Err(position) => position,
            },
        };
        // Include geometric Vec capacity growth and minimum-allocation overhead;
        // the largest set has just 25 tiny enum members, never source values.
        *retained = add(*retained, size_of::<T>() as u64 * 2 + 16)?;
        bounded(*retained, maximum)?;
        match self {
            Self::NoObservation => *self = Self::Observed(vec![grammar]),
            Self::Observed(grammars) => grammars.insert(position, grammar),
        }
        Ok(())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ScalarGrammars {
    pub decimal_strings: GrammarSet<DecimalGrammar>,
    pub decimal_numbers: GrammarSet<DecimalGrammar>,
    pub timestamps: GrammarSet<TimestampGrammar>,
}

#[derive(Debug, Default, Serialize)]
pub struct NodeSummary {
    /// Key names, with `[]` for array items; literal `[]` keys use `~[]`.
    /// A leading `~` in a key is doubled, so paths remain unambiguous.
    pub path: Vec<String>,
    pub occurrences: u64,
    pub missing: u64,
    pub types: TypeCounts,
    pub grammars: ScalarGrammars,
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
        Self::inspect_budgeted(archive, selected_entries, &mut 0, &mut 0)
    }

    pub(crate) fn inspect_budgeted<R: Read + Seek>(
        archive: &mut ArchiveInventory<'_, R>,
        selected_entries: &[EntryIndex],
        retained: &mut u64,
        tokens: &mut u64,
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
        for &index in selected_entries {
            cancelled(cancel)?;
            let path = template(archive.selected_name(index)?);
            *retained = add(*retained, 256 + path.len() as u64 * 16)?;
            bounded(*retained, limits.max_structure_bytes)?;
            let entry = archive.consume(index, |reader| {
                inspect_reader(reader, limits, cancel, retained, tokens, path)
            })?;
            entries.push(entry);
        }
        Ok(StructureReport { entries })
    }
}

// Also validates an immutable bounded header buffer before typed decoding.
// Keep the same lexical, duplicate-key, token, depth and decoded-size checks.
pub(crate) fn inspect_reader(
    reader: &mut dyn Read,
    limits: ArchiveLimits,
    cancel: &dyn Cancellation,
    retained: &mut u64,
    tokens: &mut u64,
    path: Vec<PathToken>,
) -> Result<EntryStructure, ArchiveError> {
    let failure = Cell::new(None);
    let numeric_grammar = Cell::new(DecimalGrammar::Other);
    let guard = LexicalGuard {
        inner: BufReader::with_capacity(8192, reader),
        limits,
        failure: &failure,
        depth: 0,
        in_string: false,
        escaped: false,
        scalar_bytes: 0,
        number: false,
        decimal: DecimalScanner::default(),
        numeric_grammar: &numeric_grammar,
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
        retained,
        tokens,
        numeric_grammar: &numeric_grammar,
    };
    let mut deserializer = serde_json::Deserializer::from_reader(guard);
    let parsed = ShapeSeed {
        state: &mut state,
        path: Vec::new(),
        depth: 1,
    }
    .deserialize(&mut deserializer);
    let shape = parsed.map_err(|_| failure.get().unwrap_or(ArchiveError::InvalidJson))?;
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
        shape: shape.into_report(),
        nodes: state.nodes.into_values().collect(),
        max_depth: state.max_depth,
    })
}

fn merge(target: &mut ObservedShape, incoming: ObservedShape) {
    match (target, incoming) {
        (ObservedShape::Object(left), ObservedShape::Object(right)) => {
            for (key, shape) in right {
                if let Some(previous) = left.get_mut(&key) {
                    merge(previous, shape);
                } else {
                    left.insert(key, shape);
                }
            }
        }
        (ObservedShape::Array(left), ObservedShape::Array(right)) => {
            if let Some(right) = right {
                match left {
                    Some(left) => merge(left, *right),
                    None => *left = Some(right),
                }
            }
        }
        (left, right) if *left == right => (),
        (left, _) => *left = ObservedShape::Mixed,
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
    numeric_grammar: &'a Cell<DecimalGrammar>,
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

    fn node<E: de::Error>(&mut self, path: &[String]) -> Result<&mut NodeSummary, E> {
        if !self.nodes.contains_key(path) {
            // Account for duplicated keys/paths, trees, containers and the three
            // grammar-set headers. Enum members are charged before insertion.
            let bytes = path
                .iter()
                .try_fold(256u64 + 128, |total, key| {
                    add(total, 3 * key.len() as u64 + 96)
                })
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
        Ok(self.nodes.get_mut(path).expect("inserted node"))
    }

    fn number<E: de::Error>(&mut self, path: &[String]) -> Result<(), E> {
        let grammar = self.numeric_grammar.get();
        self.node::<E>(path)?;
        let result = self
            .nodes
            .get_mut(path)
            .expect("inserted node")
            .grammars
            .decimal_numbers
            .observe(grammar, self.retained, self.limits.max_structure_bytes);
        self.check(result)
    }

    fn string<E: de::Error>(&mut self, path: &[String], value: &str) -> Result<(), E> {
        self.node::<E>(path)?;
        let grammars = &mut self.nodes.get_mut(path).expect("inserted node").grammars;
        let result = grammars
            .decimal_strings
            .observe(
                decimal_grammar(value),
                self.retained,
                self.limits.max_structure_bytes,
            )
            .and_then(|()| {
                grammars.timestamps.observe(
                    timestamp_grammar(value),
                    self.retained,
                    self.limits.max_structure_bytes,
                )
            });
        self.check(result)
    }

    fn record<E: de::Error>(&mut self, path: &[String], shape: &ObservedShape) -> Result<(), E> {
        let node = self.node(path)?;
        node.occurrences += 1;
        let count = match shape {
            ObservedShape::Null => &mut node.types.null,
            ObservedShape::Boolean => &mut node.types.boolean,
            ObservedShape::Number {
                integer: true,
                signed: false,
            } => &mut node.types.unsigned_integer,
            ObservedShape::Number {
                integer: true,
                signed: true,
            } => &mut node.types.signed_integer,
            ObservedShape::Number { integer: false, .. } => &mut node.types.number,
            ObservedShape::String => &mut node.types.string,
            ObservedShape::Array(_) => &mut node.types.array,
            ObservedShape::Object(_) => &mut node.types.object,
            ObservedShape::Mixed => return Ok(()),
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
    type Value = ObservedShape;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<ObservedShape, D::Error> {
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
    type Value = ObservedShape;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_unit<E: de::Error>(self) -> Result<ObservedShape, E> {
        self.state.scalar(1)?;
        Ok(ObservedShape::Null)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<ObservedShape, E> {
        self.state.scalar(1)?;
        Ok(ObservedShape::Boolean)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<ObservedShape, E> {
        self.state.scalar(size_of::<u64>())?;
        self.state.number(&self.path)?;
        Ok(ObservedShape::Number {
            integer: true,
            signed: false,
        })
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<ObservedShape, E> {
        self.state.scalar(size_of::<i64>())?;
        self.state.number(&self.path)?;
        Ok(ObservedShape::Number {
            integer: true,
            signed: true,
        })
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<ObservedShape, E> {
        self.state.scalar(size_of::<f64>())?;
        self.state.number(&self.path)?;
        Ok(ObservedShape::Number {
            integer: false,
            signed: value.is_sign_negative(),
        })
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<ObservedShape, E> {
        self.state.scalar(value.len())?;
        self.state.string(&self.path, value)?;
        Ok(ObservedShape::String)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<ObservedShape, A::Error> {
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
        Ok(ObservedShape::Array(shape.map(Box::new)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ObservedShape, A::Error> {
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
        Ok(ObservedShape::Object(fields))
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

#[derive(Clone, Copy, Default)]
enum DecimalState {
    #[default]
    Leading,
    Sign,
    Integer,
    BarePoint,
    IntegerPoint,
    Fraction,
    Exponent,
    ExponentSign,
    ExponentDigits,
    Trailing,
    Invalid,
}

/// Constant-space, transient syntax validation. Deliberately has no Debug or
/// Serialize implementation. Only its closed classification enters the report.
#[derive(Default)]
struct DecimalScanner {
    state: DecimalState,
    noncanonical: bool,
    leading_zero: bool,
    value: Option<u64>,
}

impl DecimalScanner {
    fn first_digit(&mut self, byte: u8) {
        self.leading_zero = byte == b'0';
        self.value = Some(u64::from(byte - b'0'));
        self.state = DecimalState::Integer;
    }

    fn push(&mut self, byte: u8) {
        use DecimalState::*;
        let whitespace = matches!(byte, b' ' | b'\t' | b'\n' | b'\r');
        match self.state {
            Leading if whitespace => self.noncanonical = true,
            Leading if matches!(byte, b'+' | b'-') => {
                self.noncanonical = true;
                self.state = Sign;
            }
            Leading | Sign if byte.is_ascii_digit() => self.first_digit(byte),
            Leading | Sign if byte == b'.' => {
                self.noncanonical = true;
                self.state = BarePoint;
            }
            Integer if byte.is_ascii_digit() => {
                self.noncanonical |= self.leading_zero;
                self.value = self
                    .value
                    .and_then(|value| value.checked_mul(10)?.checked_add(u64::from(byte - b'0')));
            }
            Integer if byte == b'.' => {
                self.noncanonical = true;
                self.state = IntegerPoint;
            }
            BarePoint | IntegerPoint | Fraction if byte.is_ascii_digit() => self.state = Fraction,
            Integer | IntegerPoint | Fraction if matches!(byte, b'e' | b'E') => {
                self.noncanonical = true;
                self.state = Exponent;
            }
            Exponent if matches!(byte, b'+' | b'-') => self.state = ExponentSign,
            Exponent | ExponentSign | ExponentDigits if byte.is_ascii_digit() => {
                self.state = ExponentDigits;
            }
            Integer | IntegerPoint | Fraction | ExponentDigits | Trailing if whitespace => {
                self.noncanonical = true;
                self.state = Trailing;
            }
            _ => self.state = Invalid,
        }
    }

    fn finish(&self) -> DecimalGrammar {
        use DecimalState::*;
        if !matches!(
            self.state,
            Integer | IntegerPoint | Fraction | ExponentDigits | Trailing
        ) {
            DecimalGrammar::Other
        } else if self.noncanonical {
            DecimalGrammar::NoncanonicalDecimal
        } else {
            match self.value {
                None => DecimalGrammar::OutOfU64Range,
                Some(0) => DecimalGrammar::Zero,
                Some(_) => DecimalGrammar::CanonicalPositiveU64Decimal,
            }
        }
    }
}

pub(crate) fn decimal_grammar(value: &str) -> DecimalGrammar {
    let mut scanner = DecimalScanner::default();
    for byte in value.bytes() {
        scanner.push(byte);
    }
    scanner.finish()
}

fn calendar_component(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0u32, |value, &byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(byte - b'0'))
    })
}

fn timestamp_grammar(value: &str) -> TimestampGrammar {
    calendar_grammar(value.as_bytes()).unwrap_or(TimestampGrammar::Unclassified)
}

fn calendar_grammar(bytes: &[u8]) -> Option<TimestampGrammar> {
    let base = bytes.get(..19)?;
    if base[4] != b'-' || base[7] != b'-' || base[13] != b':' || base[16] != b':' {
        return None;
    }
    let separator = match base[10] {
        b'T' => TimestampSeparator::UpperT,
        b' ' => TimestampSeparator::Space,
        _ => return None,
    };
    let year = calendar_component(&base[..4])?;
    let month = calendar_component(&base[5..7])?;
    let day = calendar_component(&base[8..10])?;
    let hour = calendar_component(&base[11..13])?;
    let minute = calendar_component(&base[14..16])?;
    let second = calendar_component(&base[17..19])?;
    let last_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => return None,
    };
    if year == 0 || day == 0 || day > last_day || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut suffix = &bytes[19..];
    let precision = if let Some(fraction) = suffix.strip_prefix(b".") {
        let mut matched = None;
        for (width, precision) in [
            (3, TimestampPrecision::Milliseconds),
            (6, TimestampPrecision::Microseconds),
            (9, TimestampPrecision::Nanoseconds),
        ] {
            if fraction
                .get(..width)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit))
                && !fraction.get(width).is_some_and(u8::is_ascii_digit)
            {
                suffix = &fraction[width..];
                matched = Some(precision);
                break;
            }
        }
        matched?
    } else {
        TimestampPrecision::Seconds
    };
    let zone = match suffix {
        b"" => TimestampZone::Unzoned,
        b"Z" => TimestampZone::UpperZ,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = calendar_component(&[*h1, *h2])?;
            let minutes = calendar_component(&[*m1, *m2])?;
            if hours > 23 || minutes > 59 || (*sign == b'-' && hours == 0 && minutes == 0) {
                return None;
            }
            TimestampZone::ColonOffset
        }
        _ => return None,
    };
    Some(TimestampGrammar::Calendar {
        separator,
        precision,
        zone,
    })
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
    decimal: DecimalScanner,
    numeric_grammar: &'a Cell<DecimalGrammar>,
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
                        self.decimal = DecimalScanner::default();
                    }
                    self.scalar_bytes = add(self.scalar_bytes, 1)?;
                    bounded(self.scalar_bytes, self.limits.max_scalar_bytes)?;
                    self.decimal.push(byte);
                    // The one-byte reader exposes only the current token's
                    // classification. Delimiters and key strings cannot replace
                    // it before the corresponding scalar visitor runs.
                    self.numeric_grammar.set(self.decimal.finish());
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
