mod common;
use common::*;
use discord_archive::{
    ArchiveError, ArchiveInventory, ArchiveLimits, Cancellation, ChannelContext,
    DiscordArchiveReader, DiscordId, DiscordProfile, EntryIntegrity, ProfileInspection, RecordSink,
    SentMessage,
};
use std::{
    io::{Cursor, Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const ACCOUNT: &str = r#"{"id":"9007199254741001","username":"invented_owner"}"#;
const CHANNEL: &str = r#"{"id":"9007199254741101","type":"invented_kind"}"#;
const ID: &str = "1985931830091579392";
const ROW: &str = r#"{"ID":1985931830091579392,"Timestamp":"2030-01-02 03:04:05","Contents":"invented text","Attachments":""}"#;

fn package(rows: &str) -> Vec<u8> {
    zip(&[
        ("Account/user.json", ACCOUNT.as_bytes()),
        ("Messages/index.json", b"{}"),
        (
            "Messages/c9007199254741101/channel.json",
            CHANNEL.as_bytes(),
        ),
        ("Messages/c9007199254741101/messages.json", rows.as_bytes()),
    ])
}
fn profile() -> ProfileInspection {
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(package("[]")),
        ArchiveLimits::default(),
        &NeverCancel,
    )
    .unwrap();
    DiscordProfile::detect(&mut archive).unwrap()
}
#[derive(Default)]
struct Sink {
    channels: Vec<ChannelContext>,
    messages: Vec<SentMessage>,
    ends: Vec<EntryIntegrity>,
    events: Vec<&'static str>,
}
impl RecordSink for Sink {
    fn begin_channel(&mut self, channel: ChannelContext) -> Result<(), ArchiveError> {
        self.events.push("begin");
        self.channels.push(channel);
        Ok(())
    }
    fn message(&mut self, message: SentMessage) -> Result<(), ArchiveError> {
        self.events.push("message");
        self.messages.push(message);
        Ok(())
    }
    fn end_channel(&mut self, integrity: EntryIntegrity) -> Result<(), ArchiveError> {
        self.events.push("end");
        self.ends.push(integrity);
        Ok(())
    }
}
fn read(rows: &str, limits: ArchiveLimits) -> Result<Sink, ArchiveError> {
    let archive = ArchiveInventory::inspect(Cursor::new(package(rows)), limits, &NeverCancel)?;
    let mut reader = DiscordArchiveReader::open(archive, profile(), &NeverCancel)?;
    let mut sink = Sink::default();
    reader.visit_channels(&mut sink)?;
    Ok(sink)
}

// Catches lost account/context identity, transcript buffering/order changes,
// altered Unicode or attachments, and unvalidated timezone inference.
#[test]
fn frozen_fixture_emits_exact_ordered_records_and_integrity() {
    let bytes = include_bytes!("../../../src-tauri/test-fixtures/discord-import/current-json.zip");
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes), ArchiveLimits::default(), &NeverCancel)
            .unwrap();
    let profile = DiscordProfile::detect(&mut archive).unwrap();
    let mut reader = DiscordArchiveReader::open(archive, profile, &NeverCancel).unwrap();
    let account = reader.read_account().unwrap();
    assert_eq!(account.id.as_str(), "9007199254741001");
    assert_eq!(account.username, "invented_owner");
    let mut sink = Sink::default();
    let summary = reader.visit_channels(&mut sink).unwrap();
    assert_eq!((summary.channels, summary.messages), (5, 4));
    assert_eq!(
        sink.events,
        [
            "begin", "message", "end", "begin", "message", "end", "begin", "message", "end",
            "begin", "end", "begin", "message", "end"
        ]
    );
    assert_eq!(
        sink.channels
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        [
            "9007199254741101",
            "9007199254741102",
            "9007199254741103",
            "9007199254741104",
            "9007199254741105"
        ]
    );
    assert_eq!(
        sink.channels[0].recipients.as_ref().unwrap(),
        &["9007199254741001", "9007199254741002"]
    );
    assert_eq!(sink.channels[1].name.as_deref(), Some("Invented Group"));
    assert_eq!(
        sink.channels[2].guild.as_ref().unwrap().id.as_str(),
        "9007199254741301"
    );
    assert_eq!(
        sink.channels[2].guild.as_ref().unwrap().name,
        "Invented Guild"
    );
    assert_eq!(sink.channels[3].source_type, "invented_unknown");
    assert!(sink.channels[4].name.is_none());
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../src-tauri/test-fixtures/discord-import/expected-records.json"
    ))
    .unwrap();
    for (actual, expected) in sink
        .messages
        .iter()
        .zip(expected["records"].as_array().unwrap())
    {
        assert_eq!(actual.id.as_str(), expected["messageId"].as_str().unwrap());
        assert_eq!(
            actual.account_id.as_str(),
            expected["accountId"].as_str().unwrap()
        );
        assert_eq!(
            actual.channel_id.as_str(),
            expected["channelId"].as_str().unwrap()
        );
        assert_eq!(actual.timestamp_millis, 1_893_553_445_123);
        assert_eq!(actual.contents, expected["text"].as_str().unwrap());
        assert_eq!(
            actual.attachments,
            expected["attachmentEncoding"].as_str().unwrap()
        );
    }
    assert_eq!(
        sink.messages[0].contents,
        "Invented hello 🌱\nSecond invented line."
    );
    assert_eq!(sink.messages[3].contents, "");
    assert_eq!(
        sink.ends.iter().map(|e| e.messages).collect::<Vec<_>>(),
        [1, 1, 1, 0, 1]
    );
}

#[test]
fn original_integer_tokens_and_canonical_string_ids_fail_closed() {
    for invalid in [
        "0",
        "-0",
        "-1",
        "01",
        "+1",
        "1.0",
        "1e3",
        "18446744073709551616",
        "\"1985931830091579392\"",
    ] {
        assert!(
            read(
                &format!("[{}]", ROW.replace(ID, invalid)),
                ArchiveLimits::default()
            )
            .is_err(),
            "accepted {invalid}"
        );
    }
    for invalid in [
        "",
        "0",
        "00",
        "01",
        "-1",
        "+1",
        "1.0",
        "1e3",
        " 1",
        "1 ",
        "١",
        "18446744073709551616",
    ] {
        assert!(DiscordId::parse(invalid).is_err());
    }
    assert_eq!(
        DiscordId::parse("18446744073709551615").unwrap().as_str(),
        "18446744073709551615"
    );
}

#[test]
fn unzoned_timestamp_requires_calendar_match_to_snowflake_seconds() {
    let sink = read(&format!("[{ROW}]"), ArchiveLimits::default()).unwrap();
    assert_eq!(sink.messages[0].timestamp_millis, 1_893_553_445_123);
    for value in [
        "2030-01-02 03:04:04",
        "2030-01-02 03:04:06",
        "2030-02-29 03:04:05",
        "0000-01-02 03:04:05",
        "9999-12-31 23:59:59",
        "2030-01-02 24:04:05",
        "2030-01-02 03:04:60",
        "2030-01-02T03:04:05Z",
        "2030-01-02 03:04:05.123",
    ] {
        assert!(
            read(
                &format!("[{}]", ROW.replace("2030-01-02 03:04:05", value)),
                ArchiveLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn required_fields_duplicates_and_explicit_author_fail_closed() {
    for suffix in [
        r#", "ID":1985931830091579392"#,
        r#", "Timestamp":"2030-01-02 03:04:05""#,
        r#", "Contents":"other""#,
        r#", "Attachments":"other""#,
        r#", "Author":"9007199254741002""#,
        r#", "author":{"id":"9007199254741002"}"#,
        r#", "author":null"#,
    ] {
        let row = format!("[{}{suffix}}}]", &ROW[..ROW.len() - 1]);
        assert!(serde_json::from_str::<serde_json::Value>(&row).is_ok());
        assert!(read(&row, ArchiveLimits::default()).is_err());
    }
    for field in [
        r#""ID":1985931830091579392,"#,
        r#""Timestamp":"2030-01-02 03:04:05","#,
        r#""Contents":"invented text","#,
        r#", "Attachments":"""#,
    ] {
        // Attachments uses compact spelling in the fixed record.
        let field = field.replace(", ", ",");
        assert!(
            read(
                &format!("[{}]", ROW.replace(&field, "")),
                ArchiveLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn strings_and_ignored_values_obey_raw_decoded_scalar_depth_and_token_bounds() {
    for (rows, limits) in [
        (
            format!("[{}]", ROW.replace("invented text", &"x".repeat(300))),
            ArchiveLimits {
                max_raw_record_bytes: 250,
                ..ArchiveLimits::default()
            },
        ),
        (
            format!("[{}]", ROW.replace("invented text", &"x".repeat(300))),
            ArchiveLimits {
                max_decoded_record_bytes: 200,
                ..ArchiveLimits::default()
            },
        ),
        (
            format!("[{}]", ROW.replace("invented text", &"x".repeat(300))),
            ArchiveLimits {
                max_scalar_bytes: 200,
                ..ArchiveLimits::default()
            },
        ),
        (
            format!("[{},{}]", ROW, ROW),
            ArchiveLimits {
                max_json_tokens: 55,
                ..ArchiveLimits::default()
            },
        ),
        (
            format!(
                "[{},\n{}]",
                ROW,
                ROW.replace(
                    "\"Attachments\":\"\"",
                    &format!(
                        "\"Attachments\":\"\",\"ignored\":{}0{}",
                        "[".repeat(10),
                        "]".repeat(10)
                    )
                )
            ),
            ArchiveLimits {
                max_json_depth: 8,
                ..ArchiveLimits::default()
            },
        ),
        (
            format!(
                "[{}]",
                ROW.replace(
                    "\"Attachments\":\"\"",
                    &format!("\"Attachments\":\"\",\"ignored\":\"{}\"", "x".repeat(300))
                )
            ),
            ArchiveLimits {
                max_scalar_bytes: 200,
                ..ArchiveLimits::default()
            },
        ),
    ] {
        assert!(matches!(
            read(&rows, limits),
            Err(ArchiveError::LimitExceeded)
        ));
    }
    let rows = r#"[{"ignored":[true,null,3.5,{"x":["ok"]}],"Attachments":"opaque\nhttps://example.invalid/a?x=1 https://example.invalid/b","Contents":"é 🌱\n\tsecond","Timestamp":"2030-01-02 03:04:05","ID":1985931830091579392}]"#;
    let sink = read(rows, ArchiveLimits::default()).unwrap();
    assert_eq!(sink.messages[0].contents, "é 🌱\n\tsecond");
    assert_eq!(
        sink.messages[0].attachments,
        "opaque\nhttps://example.invalid/a?x=1 https://example.invalid/b"
    );
}

#[test]
fn late_crc_fails_after_messages_without_end_event() {
    let mut bytes = package(&format!("[{ROW}]"));
    let at = bytes
        .windows(b"invented text".len())
        .position(|s| s == b"invented text")
        .unwrap();
    bytes[at] = b'x';
    let archive =
        ArchiveInventory::inspect(Cursor::new(bytes), ArchiveLimits::default(), &NeverCancel)
            .unwrap();
    let mut reader = DiscordArchiveReader::open(archive, profile(), &NeverCancel).unwrap();
    let mut sink = Sink::default();
    assert_eq!(
        reader.visit_channels(&mut sink),
        Err(ArchiveError::IntegrityFailure)
    );
    assert_eq!(sink.events, ["begin", "message"]);
}

struct Flag(AtomicBool);
impl Cancellation for Flag {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
#[test]
fn cancellation_and_sink_failure_stop_before_the_next_record() {
    struct Stop<'a> {
        flag: &'a Flag,
        count: usize,
        cancel: bool,
    }
    impl RecordSink for Stop<'_> {
        fn begin_channel(&mut self, _: ChannelContext) -> Result<(), ArchiveError> {
            Ok(())
        }
        fn message(&mut self, _: SentMessage) -> Result<(), ArchiveError> {
            self.count += 1;
            if self.cancel {
                self.flag.0.store(true, Ordering::Relaxed);
                Ok(())
            } else {
                Err(ArchiveError::ReadFailure)
            }
        }
        fn end_channel(&mut self, _: EntryIntegrity) -> Result<(), ArchiveError> {
            panic!("incomplete entry ended")
        }
    }
    for cancel in [false, true] {
        let flag = Flag(AtomicBool::new(false));
        let archive = ArchiveInventory::inspect(
            Cursor::new(package(&format!("[{ROW},{ROW}]"))),
            ArchiveLimits::default(),
            &flag,
        )
        .unwrap();
        let mut reader = DiscordArchiveReader::open(archive, profile(), &flag).unwrap();
        let mut sink = Stop {
            flag: &flag,
            count: 0,
            cancel,
        };
        assert_eq!(
            reader.visit_channels(&mut sink),
            Err(if cancel {
                ArchiveError::Cancelled
            } else {
                ArchiveError::ReadFailure
            })
        );
        assert_eq!(sink.count, 1);
        assert!(reader.visit_channels(&mut sink).is_err());
    }
}

#[test]
fn inspection_cannot_forge_schema_identity_context_or_entry_selection() {
    for mutate in 0..6 {
        let mut p = profile();
        match mutate {
            0 => p.schema_version = 9,
            1 => p.account.id = "9007199254741002".into(),
            2 => p.contexts[0].header.id = "9007199254741102".into(),
            3 => p.contexts[0].messages_entry = p.account_entry,
            4 => p.contexts.clear(),
            _ => p.contexts[0].header.source_type = "forged".into(),
        }
        let archive = ArchiveInventory::inspect(
            Cursor::new(package("[]")),
            ArchiveLimits::default(),
            &NeverCancel,
        )
        .unwrap();
        let result = DiscordArchiveReader::open(archive, p, &NeverCancel)
            .and_then(|mut reader| reader.visit_channels(&mut Sink::default()));
        assert!(result.is_err(), "accepted forged inspection {mutate}");
    }
}

struct MutableInput {
    bytes: Arc<Mutex<Vec<u8>>>,
    position: u64,
}

#[test]
fn forged_inspection_cannot_expand_the_total_selected_entry_ceiling() {
    let archive = ArchiveInventory::inspect(
        Cursor::new(package("[]")),
        ArchiveLimits {
            max_selected_contexts: 3,
            ..ArchiveLimits::default()
        },
        &NeverCancel,
    )
    .unwrap();
    assert!(matches!(
        DiscordArchiveReader::open(archive, profile(), &NeverCancel),
        Err(ArchiveError::LimitExceeded)
    ));
}

#[test]
fn distinct_reader_cancellation_is_checked_between_reads_inside_a_scalar() {
    struct Input<'a> {
        inner: Cursor<Vec<u8>>,
        flag: &'a Flag,
        armed: &'a AtomicBool,
        after: &'a std::sync::atomic::AtomicUsize,
        threshold: u64,
    }
    impl Read for Input<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(out)?;
            if self.armed.load(Ordering::Relaxed) {
                if self.flag.is_cancelled() {
                    self.after.fetch_add(count, Ordering::Relaxed);
                }
                if self.inner.position() > self.threshold {
                    self.flag.0.store(true, Ordering::Relaxed);
                }
            }
            Ok(count)
        }
    }
    impl Seek for Input<'_> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }
    for header in [false, true] {
        let bytes = if header {
            let account = format!(
                "{},\"ignored\":\"{}\"}}",
                &ACCOUNT[..ACCOUNT.len() - 1],
                "x".repeat(60_000)
            );
            zip(&[
                ("Account/user.json", account.as_bytes()),
                ("Messages/index.json", b"{}"),
                (
                    "Messages/c9007199254741101/channel.json",
                    CHANNEL.as_bytes(),
                ),
                ("Messages/c9007199254741101/messages.json", b"[]"),
            ])
        } else {
            package(&format!(
                "[{}]",
                ROW.replace("invented text", &"x".repeat(60_000))
            ))
        };
        let flag = Flag(AtomicBool::new(false));
        let armed = AtomicBool::new(false);
        let after = std::sync::atomic::AtomicUsize::new(0);
        let input = Input {
            inner: Cursor::new(bytes),
            flag: &flag,
            armed: &armed,
            after: &after,
            threshold: 20_000,
        };
        let archive =
            ArchiveInventory::inspect(input, ArchiveLimits::default(), &NeverCancel).unwrap();
        let mut reader = DiscordArchiveReader::open(archive, profile(), &flag).unwrap();
        armed.store(true, Ordering::Relaxed);
        let result = if header {
            reader.read_account().map(|_| ())
        } else {
            reader.visit_channels(&mut Sink::default()).map(|_| ())
        };
        assert_eq!(result, Err(ArchiveError::Cancelled));
        assert_eq!(
            after.load(Ordering::Relaxed),
            0,
            "read continued after cancellation inside scalar"
        );
    }
}
impl Read for MutableInput {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let bytes = self.bytes.lock().unwrap();
        let count = out
            .len()
            .min(bytes.len().saturating_sub(self.position as usize));
        out[..count]
            .copy_from_slice(&bytes[self.position as usize..self.position as usize + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for MutableInput {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.position = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(p) => self.position.checked_add_signed(p).unwrap(),
            SeekFrom::End(p) => (self.bytes.lock().unwrap().len() as u64)
                .checked_add_signed(p)
                .unwrap(),
        };
        Ok(self.position)
    }
}
#[test]
fn account_is_revalidated_from_the_retained_handle() {
    let bytes = Arc::new(Mutex::new(package("[]")));
    let archive = ArchiveInventory::inspect(
        MutableInput {
            bytes: Arc::clone(&bytes),
            position: 0,
        },
        ArchiveLimits::default(),
        &NeverCancel,
    )
    .unwrap();
    let mut reader = DiscordArchiveReader::open(archive, profile(), &NeverCancel).unwrap();
    let mut modified = bytes.lock().unwrap();
    let at = modified
        .windows(b"invented_owner".len())
        .position(|s| s == b"invented_owner")
        .unwrap();
    modified[at] = b'x';
    drop(modified);
    assert!(matches!(
        reader.read_account(),
        Err(ArchiveError::IntegrityFailure)
    ));
}

#[test]
fn retained_budget_survives_detection_but_token_work_resets_at_the_phase_boundary() {
    let account = format!(
        r#"{{"id":"9007199254741001","username":"{}"}}"#,
        "x".repeat(3000)
    );
    let bytes = zip(&[
        ("Account/user.json", account.as_bytes()),
        ("Messages/index.json", b"{}"),
        (
            "Messages/c9007199254741101/channel.json",
            CHANNEL.as_bytes(),
        ),
        ("Messages/c9007199254741101/messages.json", b"[]"),
    ]);
    let limits = ArchiveLimits {
        max_structure_bytes: 30_000,
        ..ArchiveLimits::default()
    };
    let mut archive = ArchiveInventory::inspect(Cursor::new(bytes), limits, &NeverCancel).unwrap();
    let inspection = DiscordProfile::detect(&mut archive).unwrap();
    assert!(
        matches!(
            DiscordArchiveReader::open(archive, inspection, &NeverCancel),
            Err(ArchiveError::LimitExceeded)
        ),
        "reader reset retained header allocations"
    );

    let limits = ArchiveLimits {
        max_json_tokens: 55,
        ..ArchiveLimits::default()
    };
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(package(&format!("[{ROW}]"))),
        limits,
        &NeverCancel,
    )
    .unwrap();
    let inspection = DiscordProfile::detect(&mut archive).unwrap();
    let mut reader = DiscordArchiveReader::open(archive, inspection, &NeverCancel).unwrap();
    assert_eq!(
        reader
            .visit_channels(&mut Sink::default())
            .unwrap()
            .messages,
        1
    );
}

// Real allocation observations make a transcript Vec visible even when a sink
// is synchronous. Fixture construction/detection happen before measurement.
struct AllocationObserver;
std::thread_local! {static WATCH: std::cell::Cell<bool> = const {std::cell::Cell::new(false)}; static LIVE: std::cell::Cell<isize> = const {std::cell::Cell::new(0)}; static PEAK: std::cell::Cell<isize> = const {std::cell::Cell::new(0)};}
fn allocated(delta: isize) {
    if WATCH.try_with(|v| v.get()).unwrap_or(false) {
        let _ = LIVE.try_with(|n| {
            n.set(n.get() + delta);
            let _ = PEAK.try_with(|p| p.set(p.get().max(n.get())));
        });
    }
}
unsafe impl std::alloc::GlobalAlloc for AllocationObserver {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        allocated(layout.size() as isize);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        allocated(-(layout.size() as isize));
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        allocated(size as isize - layout.size() as isize);
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationObserver = AllocationObserver;

#[test]
fn generated_100000_records_have_bounded_live_heap_and_sink_in_flight() {
    struct Count {
        messages: usize,
        active: usize,
        maximum: usize,
    }
    impl RecordSink for Count {
        fn begin_channel(&mut self, _: ChannelContext) -> Result<(), ArchiveError> {
            Ok(())
        }
        fn message(&mut self, message: SentMessage) -> Result<(), ArchiveError> {
            self.active += 1;
            self.maximum = self.maximum.max(self.active);
            assert_eq!(message.contents.len(), 1024);
            self.messages += 1;
            drop(message);
            self.active -= 1;
            Ok(())
        }
        fn end_channel(&mut self, integrity: EntryIntegrity) -> Result<(), ArchiveError> {
            assert_eq!(integrity.messages as usize, self.messages);
            Ok(())
        }
    }
    let mut peaks = Vec::new();
    for count in [10_000, 100_000] {
        let row = ROW.replace("invented text", &"x".repeat(1024));
        let rows = format!(
            "[{}]",
            std::iter::repeat_n(row.as_str(), count)
                .collect::<Vec<_>>()
                .join(",")
        );
        let archive = ArchiveInventory::inspect(
            Cursor::new(package(&rows)),
            ArchiveLimits::default(),
            &NeverCancel,
        )
        .unwrap();
        drop(rows);
        let mut reader = DiscordArchiveReader::open(archive, profile(), &NeverCancel).unwrap();
        let mut sink = Count {
            messages: 0,
            active: 0,
            maximum: 0,
        };
        LIVE.with(|v| v.set(0));
        PEAK.with(|v| v.set(0));
        WATCH.with(|v| v.set(true));
        let result = reader.visit_channels(&mut sink);
        WATCH.with(|v| v.set(false));
        let peak = PEAK.with(|v| v.get());
        peaks.push(peak);
        assert_eq!(result.unwrap().messages as usize, count);
        assert_eq!((sink.messages, sink.maximum, sink.active), (count, 1, 0));
        assert!(peak < 1_000_000, "reader peak live heap {peak}");
        eprintln!(
            "records={count}, peak additional live heap={peak}, maximum sink in-flight={}",
            sink.maximum
        );
    }
    assert!(peaks[1] <= peaks[0] + 64_000);
}
