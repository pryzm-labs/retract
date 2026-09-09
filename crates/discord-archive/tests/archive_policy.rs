mod common;
use common::*;
use discord_archive::{ArchiveError, ArchiveInventory, Cancellation, EntryIndex};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn rejected(bytes: Vec<u8>, expected: ArchiveError) {
    assert_eq!(
        ArchiveInventory::inspect(Cursor::new(bytes), limits(), &NeverCancel).unwrap_err(),
        expected
    );
}

#[test]
fn traversal_is_rejected_before_payload_read() {
    struct Guard(Cursor<Vec<u8>>);
    impl Read for Guard {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let at = self.0.position() as usize;
            let payload = 30 + "messages/../../secret.json".len();
            // Preflight may inspect the bounded EOCD tail, but never decompresses payloads.
            assert!(!(at == payload && out.len() <= 2));
            self.0.read(out)
        }
    }
    impl Seek for Guard {
        fn seek(&mut self, at: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(at)
        }
    }
    let bytes = zip(&[("messages/../../secret.json", b"{}")]);
    let error = ArchiveInventory::inspect(Guard(Cursor::new(bytes)), limits(), &NeverCancel)
        .err()
        .unwrap();
    assert_eq!(error, ArchiveError::UnsafeEntryName);
}

#[test]
fn unsafe_and_nonportable_names_fail_closed() {
    for name in [
        "/absolute",
        "../x",
        "a/../x",
        "a/./x",
        "a//x",
        "a\\x",
        "C:x",
        "C:/x",
        "\\\\host\\x",
        "a\0b",
        "a\nb",
        "a\u{7f}b",
        "café",
        "cafe\u{301}",
        "x. ",
        "x.",
        "x /y",
    ] {
        rejected(zip(&[(name, b"")]), ArchiveError::UnsafeEntryName);
    }
    let mut bytes = zip(&[("x", b"")]);
    bytes[30] = 0xff;
    let at = central(&bytes);
    bytes[at + 46] = 0xff;
    rejected(bytes, ArchiveError::UnsafeEntryName);
}

#[test]
fn duplicate_case_and_file_directory_conflicts_are_rejected() {
    for names in [
        ["a", "a"],
        ["A", "a"],
        ["a", "a/"],
        ["a", "a/b"],
        ["A/x", "a/y"],
        ["a/b", "a"],
    ] {
        rejected(
            zip(&[(names[0], b""), (names[1], b"")]),
            ArchiveError::UnsafeEntryName,
        );
    }
}

#[test]
fn encryption_and_unsupported_compression_are_rejected() {
    for flag in [1, 0x40, 0x2000] {
        let mut bytes = zip(&[("x", b"")]);
        let at = central(&bytes);
        put16(&mut bytes, at + 8, flag);
        put16(&mut bytes, 6, flag);
        rejected(bytes, ArchiveError::UnsupportedFeature);
    }
    let mut bytes = zip(&[("x", b"")]);
    let at = central(&bytes);
    put16(&mut bytes, at + 10, 12);
    put16(&mut bytes, 8, 12);
    rejected(bytes, ArchiveError::UnsupportedFeature);
}

#[test]
fn links_special_modes_and_directory_payloads_are_rejected() {
    for mode in [0o120777, 0o010644, 0o020644, 0o060644, 0o140644, 0o040755] {
        let mut bytes = zip(&[("x", b"")]);
        let at = central(&bytes);
        put32(&mut bytes, at + 38, mode << 16);
        rejected(bytes, ArchiveError::UnsupportedFeature);
    }
    rejected(zip(&[("a/", b"x")]), ArchiveError::UnsupportedFeature);
}

#[test]
fn multidisk_prefixed_truncated_and_trailing_archives_are_rejected() {
    let mut bytes = zip(&[("x", b"")]);
    let at = eocd(&bytes);
    put16(&mut bytes, at + 4, 1);
    rejected(bytes, ArchiveError::UnsupportedFeature);
    let mut bytes = zip(&[("x", b"")]);
    bytes.insert(0, 0);
    rejected(bytes, ArchiveError::InvalidArchive);
    let mut bytes = zip(&[("x", b"")]);
    bytes.truncate(bytes.len() - 5);
    rejected(bytes, ArchiveError::InvalidArchive);
    let mut bytes = zip(&[("x", b"")]);
    bytes.push(0);
    rejected(bytes, ArchiveError::InvalidArchive);
}

#[test]
fn local_and_central_headers_must_agree() {
    for (at, value) in [
        (4, 10),
        (6, 8),
        (8, 8),
        (10, 1),
        (12, 1),
        (14, 1),
        (18, 3),
        (22, 3),
        (30, b'y'),
    ] {
        let mut bytes = zip(&[("x", b"{}")]);
        bytes[at] = value;
        rejected(bytes, ArchiveError::InvalidArchive);
    }
}

#[test]
fn embedded_end_markers_cannot_redirect_library_directory_fallback() {
    let mut bytes = zip(&[("x", b"{}")]);
    let at = central(&bytes);
    let end = eocd(&bytes);
    let extra = [0xff, 0xff, 4, 0, b'P', b'K', 5, 6];
    bytes.splice(end..end, extra);
    put16(&mut bytes, at + 30, 8);
    put32(&mut bytes, end + 8 + 12, 55);
    rejected(bytes, ArchiveError::InvalidArchive);
}

#[test]
fn metadata_extra_fields_and_dos_special_modes_fail_closed() {
    for extra in [[1, 0, 0, 0], [1, 0, 20, 0]] {
        let mut bytes = zip(&[("x", b"")]);
        let at = central(&bytes);
        let end = eocd(&bytes);
        bytes.splice(end..end, extra);
        put16(&mut bytes, at + 30, 4);
        put32(&mut bytes, end + 4 + 12, 51);
        rejected(bytes, ArchiveError::UnsupportedZip64);
    }
    let mut bytes = zip(&[("x", b"")]);
    let at = central(&bytes);
    bytes[at + 5] = 0;
    put32(&mut bytes, at + 38, 8);
    rejected(bytes, ArchiveError::UnsupportedFeature);
}

#[test]
fn lied_expanded_sizes_cannot_validate_either_codec() {
    use std::io::Write;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("x", SimpleFileOptions::default().compression_method(method))
            .unwrap();
        writer.write_all(b"{} {}").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        let at = central(&bytes);
        put32(&mut bytes, 22, 1);
        put32(&mut bytes, at + 24, 1);
        let mut archive =
            ArchiveInventory::inspect(Cursor::new(bytes), limits(), &NeverCancel).unwrap();
        assert_eq!(
            archive.validate_entry(EntryIndex(0)),
            Err(ArchiveError::IntegrityFailure)
        );
        assert!(!archive.is_validated(EntryIndex(0)));
    }
}

#[test]
fn overlapping_and_out_of_file_ranges_are_rejected() {
    let mut bytes = zip(&[("x", b"{}"), ("y", b"{}")]);
    let first = central(&bytes);
    let second = first + 47;
    put32(&mut bytes, second + 42, 0);
    rejected(bytes, ArchiveError::InvalidArchive);
    let mut bytes = zip(&[("x", b"{}")]);
    let at = central(&bytes);
    put32(&mut bytes, at + 42, u32::MAX - 1);
    rejected(bytes, ArchiveError::InvalidArchive);
}

#[test]
fn a_valid_local_header_inside_another_payload_is_still_an_overlap() {
    let inner = zip(&[("inner", b"x")]);
    let inner_local = &inner[..central(&inner)];
    let mut bytes = zip(&[("outer", inner_local), ("inner", b"x")]);
    let second = central(&bytes) + 51;
    put32(&mut bytes, second + 42, 35);
    rejected(bytes, ArchiveError::InvalidArchive);
}

#[test]
fn truncated_central_entries_and_data_descriptors_fail_closed() {
    let mut bytes = zip(&[("x", b"{}")]);
    let at = central(&bytes);
    put16(&mut bytes, at + 28, 200);
    rejected(bytes, ArchiveError::InvalidArchive);
    let mut bytes = zip(&[("x", b"{}")]);
    let at = central(&bytes);
    put16(&mut bytes, at + 8, 8);
    put16(&mut bytes, 6, 8);
    rejected(bytes, ArchiveError::UnsupportedFeature);
}

#[test]
fn crc_failure_never_marks_an_entry_valid() {
    let mut bytes = zip(&[("x", b"{}")]);
    bytes[31] = b'!';
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes), limits(), &NeverCancel).unwrap();
    assert_eq!(
        archive.validate_entry(EntryIndex(0)),
        Err(ArchiveError::IntegrityFailure)
    );
    assert!(!archive.is_validated(EntryIndex(0)));
}

#[test]
fn zip64_is_rejected_including_contradictory_forms() {
    for field in [8, 10] {
        let mut bytes = zip(&[("x", b"")]);
        let at = eocd(&bytes);
        put16(&mut bytes, at + field, u16::MAX);
        rejected(bytes, ArchiveError::UnsupportedZip64);
    }
    for field in [12, 16] {
        let mut bytes = zip(&[("x", b"")]);
        let at = eocd(&bytes);
        put32(&mut bytes, at + field, u32::MAX);
        rejected(bytes, ArchiveError::UnsupportedZip64);
    }
    let mut bytes = zip(&[("x", b"")]);
    let at = central(&bytes);
    put32(&mut bytes, at + 24, u32::MAX);
    rejected(bytes, ArchiveError::UnsupportedZip64);
}

#[test]
fn every_declared_limit_is_enforced_before_consumption() {
    let bytes = zip(&[("messages/a.json", b"{}"), ("other", b"{}")]);
    let mut cases = Vec::new();
    let mut l = limits();
    l.max_archive_bytes = 1;
    cases.push(l);
    let mut l = limits();
    l.max_directory_bytes = 1;
    cases.push(l);
    let mut l = limits();
    l.max_entries = 1;
    cases.push(l);
    let mut l = limits();
    l.max_path_bytes = 2;
    cases.push(l);
    let mut l = limits();
    l.max_path_components = 1;
    cases.push(l);
    let mut l = limits();
    l.max_entry_bytes = 1;
    cases.push(l);
    let mut l = limits();
    l.max_total_declared_bytes = 3;
    cases.push(l);
    for l in cases {
        assert_eq!(
            ArchiveInventory::inspect(Cursor::new(bytes.clone()), l, &NeverCancel).unwrap_err(),
            ArchiveError::LimitExceeded
        );
    }
}

#[test]
fn zero_compressed_nonempty_and_excessive_ratio_are_rejected() {
    for (compressed, expanded) in [(0, 1), (1, 101)] {
        let mut bytes = zip(&[("x", b"x")]);
        let at = central(&bytes);
        put32(&mut bytes, at + 20, compressed);
        put32(&mut bytes, 18, compressed);
        put32(&mut bytes, at + 24, expanded);
        put32(&mut bytes, 22, expanded);
        rejected(bytes, ArchiveError::LimitExceeded);
    }
}

#[test]
fn cancellation_applies_to_inventory_and_consumption() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Flag(AtomicBool);
    impl Cancellation for Flag {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let flag = Flag(AtomicBool::new(true));
    let bytes = zip(&[("x", b"{}")]);
    assert_eq!(
        ArchiveInventory::inspect(Cursor::new(bytes.clone()), limits(), &flag).unwrap_err(),
        ArchiveError::Cancelled
    );
    flag.0.store(false, Ordering::Relaxed);
    let mut archive = ArchiveInventory::inspect(Cursor::new(bytes), limits(), &flag).unwrap();
    flag.0.store(true, Ordering::Relaxed);
    assert_eq!(
        archive.validate_entry(EntryIndex(0)),
        Err(ArchiveError::Cancelled)
    );
    assert!(!archive.is_validated(EntryIndex(0)));
}

#[test]
fn arithmetic_overflow_and_limits_above_design_ceilings_are_rejected() {
    let mut l = limits();
    l.max_expansion_ratio = u64::MAX;
    assert_eq!(
        ArchiveInventory::inspect(Cursor::new(zip(&[("x", b"{}")])), l, &NeverCancel).unwrap_err(),
        ArchiveError::InvalidLimits
    );
    let mut bytes = zip(&[("x", b"")]);
    let at = eocd(&bytes);
    put32(&mut bytes, at + 12, u32::MAX - 1);
    put32(&mut bytes, at + 16, u32::MAX - 1);
    rejected(bytes, ArchiveError::InvalidArchive);
}

#[test]
fn observed_pass_budget_includes_repeated_consumption() {
    let mut l = limits();
    l.max_observed_bytes = 3;
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(zip(&[("x", b"{}")])), l, &NeverCancel).unwrap();
    archive.validate_entry(EntryIndex(0)).unwrap();
    assert!(archive.is_validated(EntryIndex(0)));
    assert_eq!(
        archive.validate_entry(EntryIndex(0)),
        Err(ArchiveError::LimitExceeded)
    );
    assert!(!archive.is_validated(EntryIndex(0)));
}

#[test]
fn ordinary_directories_stored_and_deflated_entries_are_supported() {
    use std::io::Write;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_directory("messages/", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file(
            "messages/a.json",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(b"{}").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes), limits(), &NeverCancel).unwrap();
    assert_eq!(archive.entry_count(), 2);
    archive.validate_entry(EntryIndex(1)).unwrap();
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(zip(&[("x", b"{}")])), limits(), &NeverCancel)
            .unwrap();
    archive.validate_entry(EntryIndex(0)).unwrap();
    assert_eq!(
        archive.validate_entry(EntryIndex(9)),
        Err(ArchiveError::InvalidSelection)
    );
}
