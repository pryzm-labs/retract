mod common;
use common::*;
use discord_archive::{
    ArchiveError, ArchiveInventory, ArchiveLimits, Cancellation, ChannelContext,
    DiscordArchiveReader, DiscordProfile, EntryIntegrity, RecordSink, SentMessage,
};
use std::{
    cell::Cell,
    io::{Cursor, Read, Seek, SeekFrom},
};

const ACCOUNT: &str = r#"{"id":"9007199254741001","username":"invented_owner"}"#;
const CHANNEL: &str = r#"{"id":"9007199254741101","type":"invented_kind"}"#;
const CHANNEL_PATH: &str = "Messages/c9007199254741101/channel.json";
const MESSAGES_PATH: &str = "Messages/c9007199254741101/messages.json";

#[derive(Clone, Copy, Default)]
struct Observation {
    mode: u8,
    loaded: bool,
    requested: bool,
    typed: bool,
    polls: usize,
    largest_after_request: usize,
    input_after_request: usize,
}
thread_local! {static OBSERVED: Cell<Observation> = const {Cell::new(Observation {
    mode: 0, loaded: false, requested: false, typed: false, polls: 0,
    largest_after_request: 0, input_after_request: 0,
})};}

// Allocation events distinguish buffered parsing from archive reads without
// sleeps, thread races, production hooks or changing allocator behavior.
fn observe_allocation(size: usize) {
    let _ = OBSERVED.try_with(|state| {
        let mut value = state.get();
        if value.mode == 1 && value.loaded && size == 16_384 {
            // The raw header buffer is already loaded. Its capacities are powers
            // of two above 64 KiB; this is Serde's growing scalar scratch buffer.
            value.requested = true;
        }
        if value.mode == 2 && value.loaded && size == 3000 {
            // Only the typed display-string copy has this exact size. Structural
            // parsing retains no display value and uses geometric scratch growth.
            value.typed = true;
        }
        if value.requested {
            value.largest_after_request = value.largest_after_request.max(size);
        }
        state.set(value);
    });
}
struct Observer;
unsafe impl std::alloc::GlobalAlloc for Observer {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        observe_allocation(size);
        unsafe { std::alloc::System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Observer = Observer;

struct Signal;
impl Cancellation for Signal {
    fn is_cancelled(&self) -> bool {
        OBSERVED.with(|state| {
            let mut value = state.get();
            if value.mode == 2 && value.typed {
                value.polls += 1;
                if value.polls >= 16 {
                    value.requested = true;
                }
            }
            state.set(value);
            value.requested
        })
    }
}
struct Input {
    inner: Cursor<Vec<u8>>,
    header_end: u64,
    compressed_start: Option<u64>,
}
impl Read for Input {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.position();
        let count = self.inner.read(output)?;
        let end = self.inner.position();
        OBSERVED.with(|state| {
            let mut value = state.get();
            if value.mode != 0 {
                if value.requested {
                    value.input_after_request += count;
                }
                if count > 0 && end == self.header_end {
                    value.loaded = true;
                }
                if value.mode == 3
                    && count > 0
                    && self.compressed_start.is_some_and(|at| start >= at)
                {
                    value.requested = true;
                }
            }
            state.set(value);
        });
        Ok(count)
    }
}
impl Seek for Input {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}
#[derive(Default)]
struct Sink {
    begun: usize,
}
impl RecordSink for Sink {
    fn begin_channel(&mut self, _: ChannelContext) -> Result<(), ArchiveError> {
        self.begun += 1;
        Ok(())
    }
    fn message(&mut self, _: SentMessage) -> Result<(), ArchiveError> {
        panic!("unexpected record")
    }
    fn end_channel(&mut self, _: EntryIntegrity) -> Result<(), ArchiveError> {
        Ok(())
    }
}

fn buffered_header_case(channel: bool, mode: u8, original_signal: bool) {
    let display = if mode == 2 {
        "x".repeat(3000)
    } else {
        "invented".into()
    };
    let ignored = if mode == 1 {
        format!("\"{}\"", "x".repeat(100_000))
    } else {
        format!("[{}0]", "0,".repeat(100_000))
    };
    let header = format!(
        "{{\"id\":\"{}\",\"{}\":\"{display}\",\"ignored\":{ignored}}}",
        if channel {
            "9007199254741101"
        } else {
            "9007199254741001"
        },
        if channel { "type" } else { "username" }
    );
    let account = if channel { ACCOUNT } else { &header };
    let context = if channel { &header } else { CHANNEL };
    let bytes = zip(&[
        ("Account/user.json", account.as_bytes()),
        ("Messages/index.json", b"{}"),
        (CHANNEL_PATH, context.as_bytes()),
        (MESSAGES_PATH, b"[]"),
    ]);
    let header_start = bytes
        .windows(header.len())
        .position(|s| s == header.as_bytes())
        .unwrap();
    OBSERVED.with(|state| state.set(Observation::default()));
    let input = Input {
        inner: Cursor::new(bytes),
        header_end: (header_start + header.len()) as u64,
        compressed_start: None,
    };
    // Exercise each cancellation authority independently. No event is armed
    // before the typed reader operation starts its buffered parsing passes.
    let mut archive = ArchiveInventory::inspect(
        input,
        ArchiveLimits::default(),
        if original_signal {
            &Signal
        } else {
            &NeverCancel
        },
    )
    .unwrap();
    let profile = DiscordProfile::detect(&mut archive).unwrap();
    let mut reader = DiscordArchiveReader::open(
        archive,
        profile,
        if original_signal {
            &NeverCancel
        } else {
            &Signal
        },
    )
    .unwrap();
    let mut sink = Sink::default();
    OBSERVED.with(|state| {
        state.set(Observation {
            mode,
            ..Observation::default()
        })
    });
    let result = if channel {
        reader.visit_channels(&mut sink).map(|_| ())
    } else {
        reader.read_account().map(|_| ())
    };
    let observed = OBSERVED.with(|state| {
        let value = state.get();
        state.set(Observation::default());
        value
    });
    assert_eq!(
        result,
        Err(ArchiveError::Cancelled),
        "buffered pass did not observe cancellation"
    );
    assert!(observed.loaded && observed.requested);
    assert_eq!(
        observed.input_after_request, 0,
        "cancellation must occur after loading the header"
    );
    assert_eq!(sink.begun, 0, "cancelled header must not publish a channel");
    if mode == 1 {
        assert!(
            observed.largest_after_request <= 16_384,
            "buffered scalar continued allocating: {}",
            observed.largest_after_request
        );
    } else {
        assert!(
            observed.typed,
            "cancellation must occur in the field-selective buffered pass"
        );
    }
}
#[test]
fn cancellation_during_buffered_header_scalar_stops_scratch_growth() {
    for original in [false, true] {
        for channel in [false, true] {
            buffered_header_case(channel, 1, original);
        }
    }
}
#[test]
fn cancellation_during_buffered_ignored_subtree_prevents_header_completion() {
    for original in [false, true] {
        for channel in [false, true] {
            buffered_header_case(channel, 2, original);
        }
    }
}

#[test]
fn distinct_reader_cancellation_reaches_reads_below_the_deflate_decoder() {
    for deflated in [false, true] {
        for original_signal in [false, true] {
            // Valid raw deflate with many non-final empty stored blocks before the final
            // JSON block. A decoder may need many compressed reads for one output read.
            let mut bytes = zip(&[
                ("Account/user.json", ACCOUNT.as_bytes()),
                ("Messages/index.json", b"{}"),
                (CHANNEL_PATH, CHANNEL.as_bytes()),
                (MESSAGES_PATH, b"[]"),
            ]);
            let old_central = bytes.windows(4).rposition(|s| s == b"PK\x01\x02").unwrap();
            let local = u32::from_le_bytes(
                bytes[old_central + 42..old_central + 46]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let data = local + 30 + MESSAGES_PATH.len();
            let mut compressed = [0, 0, 0, 255, 255].repeat(20_000);
            compressed.extend_from_slice(&[1, 2, 0, 253, 255, b'[', b']']);
            let length = compressed.len();
            if deflated {
                bytes.splice(data..data + 2, compressed);
                let central = old_central + length - 2;
                put16(&mut bytes, local + 8, 8);
                put16(&mut bytes, central + 10, 8);
                put32(&mut bytes, local + 18, length as u32);
                put32(&mut bytes, central + 20, length as u32);
                let end = eocd(&bytes);
                let directory = data + length;
                put32(&mut bytes, end + 16, directory as u32);
            }
            OBSERVED.with(|state| state.set(Observation::default()));
            let input = Input {
                inner: Cursor::new(bytes),
                header_end: 0,
                compressed_start: Some(data as u64),
            };
            let mut archive = ArchiveInventory::inspect(
                input,
                ArchiveLimits::default(),
                if original_signal {
                    &Signal
                } else {
                    &NeverCancel
                },
            )
            .unwrap();
            let profile = DiscordProfile::detect(&mut archive).unwrap(); // proves valid deflate/CRC
            let mut reader = DiscordArchiveReader::open(
                archive,
                profile,
                if original_signal {
                    &NeverCancel
                } else {
                    &Signal
                },
            )
            .unwrap();
            OBSERVED.with(|state| {
                state.set(Observation {
                    mode: 3,
                    ..Observation::default()
                })
            });
            let result = reader.visit_channels(&mut Sink::default());
            let observed = OBSERVED.with(|state| {
                let value = state.get();
                state.set(Observation::default());
                value
            });
            assert_eq!(result, Err(ArchiveError::Cancelled));
            assert!(observed.requested);
            assert_eq!(
                observed.input_after_request, 0,
                "decoder continued compressed input reads after cancellation"
            );
        }
    }
}
