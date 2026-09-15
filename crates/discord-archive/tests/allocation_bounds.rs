mod common;

use common::{NeverCancel, zip};
use discord_archive::{ArchiveError, ArchiveInventory, ArchiveLimits, EntryIndex, StructureProbe};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    io::Cursor,
};

// Observe the real allocator on this test thread; never fail an allocation or
// modify production behavior. Fixture creation is outside the measured region.
struct AllocationObserver;
thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn allocated(delta: isize) {
    if WATCH.try_with(Cell::get).unwrap_or(false) {
        let _ = LIVE.try_with(|live| {
            live.set(live.get() + delta);
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
        });
    }
}

unsafe impl GlobalAlloc for AllocationObserver {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size() as isize);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        allocated(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            // Conservatively include old and new capacity at realloc's peak.
            allocated(size as isize);
            allocated(-(layout.size() as isize));
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: AllocationObserver = AllocationObserver;

fn peak_live(run: impl FnOnce()) -> isize {
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            WATCH.with(|watch| watch.set(false));
        }
    }
    LIVE.with(|live| live.set(0));
    PEAK.with(|peak| peak.set(0));
    WATCH.with(|watch| watch.set(true));
    let stop = Stop;
    run();
    drop(stop);
    PEAK.with(Cell::get)
}

fn inventory_peak(shared_prefix: bool) -> isize {
    let names: Vec<_> = (0..128)
        .map(|index| {
            let first = if shared_prefix {
                "a".repeat(220)
            } else {
                format!("{index:04}{}", "a".repeat(216))
            };
            format!("{first}{}{index:04}", "/x".repeat(14) + "/")
        })
        .collect();
    let entries: Vec<_> = names
        .iter()
        .map(|name| (name.as_str(), b"".as_slice()))
        .collect();
    let input = zip(&entries);
    let limits = ArchiveLimits {
        max_archive_bytes: 100_000,
        max_directory_bytes: 40_000,
        max_entries: 128,
        max_path_bytes: 256,
        max_path_components: 16,
        ..ArchiveLimits::default()
    };
    peak_live(|| {
        let archive =
            ArchiveInventory::inspect(Cursor::new(input.as_slice()), limits, &NeverCancel).unwrap();
        assert_eq!(archive.entry_count(), 128);
    })
}

#[test]
fn inventory_unique_long_prefixes_do_not_amplify_the_directory_budget() {
    // Restoring two full owned prefixes per component exceeds this live bound,
    // even though every input and injected ZIP ceiling remains valid.
    let peak = inventory_peak(false);
    assert!(peak < 512 * 1024, "unique-prefix peak live bytes: {peak}");
    eprintln!("unique-prefix peak live bytes: {peak}");
}

#[test]
fn inventory_shared_prefixes_remain_valid_and_bounded() {
    let peak = inventory_peak(true);
    assert!(peak < 512 * 1024, "shared-prefix peak live bytes: {peak}");
    eprintln!("shared-prefix peak live bytes: {peak}");
}

fn nested_probe_peak(arrays: bool) -> (Result<(), ArchiveError>, isize) {
    let first = "a".repeat(8192);
    let second = format!("~{}", "b".repeat(8191));
    let mut json = format!("{{\"{first}\":{{\"{second}\":");
    json.push_str(&if arrays {
        "[".repeat(38)
    } else {
        "{\"x\":".repeat(38)
    });
    json.push('0');
    json.push_str(&if arrays {
        "]".repeat(38)
    } else {
        "}".repeat(38)
    });
    json.push_str("}}");
    let input = zip(&[("data.json", json.as_bytes())]);
    let limits = ArchiveLimits {
        max_structure_bytes: 32 * 1024,
        max_scalar_bytes: 8192,
        max_json_depth: 48,
        max_raw_record_bytes: 32 * 1024,
        max_decoded_record_bytes: 24 * 1024,
        ..ArchiveLimits::default()
    };
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(input.as_slice()), limits, &NeverCancel).unwrap();
    let mut result = Ok(());
    let peak = peak_live(|| {
        result = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).map(|_| ());
    });
    (result, peak)
}

#[test]
fn nested_object_paths_reject_before_live_ancestor_copy_amplification() {
    let (result, peak) = nested_probe_peak(false);
    // Eventual rejection alone already passed before the fix. The peak bound
    // specifically catches allocations made before the first node is charged.
    assert!(peak < 128 * 1024, "nested-object peak live bytes: {peak}");
    assert_eq!(result, Err(ArchiveError::LimitExceeded));
    eprintln!("nested-object peak live bytes: {peak}");
}

#[test]
fn nested_array_paths_reject_before_live_ancestor_copy_amplification() {
    let (result, peak) = nested_probe_peak(true);
    assert!(peak < 128 * 1024, "nested-array peak live bytes: {peak}");
    assert_eq!(result, Err(ArchiveError::LimitExceeded));
    eprintln!("nested-array peak live bytes: {peak}");
}
