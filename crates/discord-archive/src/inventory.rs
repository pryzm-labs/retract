use crate::{
    ArchiveError, ArchiveLimits,
    limits::{add, bounded, ratio},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fmt,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use zip::ZipArchive;

pub trait Cancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntryIndex(pub usize);

struct Entry {
    name: String,
    expanded: u64,
    compressed: u64,
    directory: bool,
    validated: bool,
}

/// Owns the retained input. Debug output deliberately omits input and names.
pub struct ArchiveInventory<'a, R> {
    archive: ZipArchive<InventoryReader<'a, R>>,
    entries: Vec<Entry>,
    pub(crate) limits: ArchiveLimits,
    pub(crate) cancel: &'a dyn Cancellation,
    observed: u64,
    reader_cancel: Arc<OnceLock<&'a dyn Cancellation>>,
}

impl<R> fmt::Debug for ArchiveInventory<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArchiveInventory")
            .field("entry_count", &self.entries.len())
            .finish_non_exhaustive()
    }
}

pub(crate) fn cancelled(cancel: &dyn Cancellation) -> Result<(), ArchiveError> {
    if cancel.is_cancelled() {
        Err(ArchiveError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) struct EitherCancellation<'a>(pub &'a dyn Cancellation, pub &'a dyn Cancellation);
impl Cancellation for EitherCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled() || self.1.is_cancelled()
    }
}

/// Use at every parsing layer, including immutable buffered passes. Keeps
/// cancellation distinguishable from JSON/decoder errors without source data.
pub(crate) struct CancelRead<'a> {
    reader: &'a mut dyn Read,
    cancel: &'a dyn Cancellation,
    failure: &'a Cell<Option<ArchiveError>>,
}
impl<'a> CancelRead<'a> {
    pub(crate) fn new(
        reader: &'a mut dyn Read,
        cancel: &'a dyn Cancellation,
        failure: &'a Cell<Option<ArchiveError>>,
    ) -> Self {
        Self {
            reader,
            cancel,
            failure,
        }
    }
}
impl Read for CancelRead<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        cancelled(self.cancel).map_err(|error| {
            self.failure.set(Some(error));
            io::Error::other(error)
        })?;
        let length = bytes.len().min(8192);
        self.reader.read(&mut bytes[..length])
    }
}

fn entry_io_error(error: io::Error) -> ArchiveError {
    if error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ArchiveError>())
        == Some(&ArchiveError::Cancelled)
    {
        ArchiveError::Cancelled
    } else {
        ArchiveError::IntegrityFailure
    }
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    size: usize,
    cancel: &dyn Cancellation,
) -> Result<Vec<u8>, ArchiveError> {
    cancelled(cancel)?;
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| ArchiveError::ReadFailure)?;
    let mut bytes = vec![0; size];
    for chunk in bytes.chunks_mut(8192) {
        cancelled(cancel)?;
        reader
            .read_exact(chunk)
            .map_err(|_| ArchiveError::InvalidArchive)?;
    }
    Ok(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn extra_fields(mut bytes: &[u8]) -> Result<(), ArchiveError> {
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(ArchiveError::InvalidArchive);
        }
        let tag = u16_at(bytes, 0);
        let length = usize::from(u16_at(bytes, 2));
        if tag == 1 {
            return Err(ArchiveError::UnsupportedZip64);
        }
        // Unicode path overrides and encryption metadata must not reinterpret names/data.
        if matches!(tag, 0x7075 | 0x6375 | 0x9901 | 0x0017) {
            return Err(ArchiveError::UnsupportedFeature);
        }
        bytes = bytes
            .get(4 + length..)
            .ok_or(ArchiveError::InvalidArchive)?;
    }
    Ok(())
}

// Component bytes borrow the already bounded directory; comparison folds ASCII
// without allocating. Each distinct (parent, component) stores only fixed-size
// metadata, never an owned copy of every ancestor prefix.
struct AsciiComponent<'a>(&'a str);

impl PartialEq for AsciiComponent<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}
impl Eq for AsciiComponent<'_> {}
impl PartialOrd for AsciiComponent<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for AsciiComponent<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(other.0.bytes().map(|byte| byte.to_ascii_lowercase()))
    }
}

struct PathNode {
    id: usize,
    directory: bool,
    explicit: bool,
}

fn validate_name<'a>(
    raw: &'a [u8],
    limits: ArchiveLimits,
    paths: &mut BTreeMap<(usize, AsciiComponent<'a>), PathNode>,
) -> Result<String, ArchiveError> {
    bounded(raw.len() as u64, limits.max_path_bytes)?;
    let name = std::str::from_utf8(raw).map_err(|_| ArchiveError::UnsafeEntryName)?;
    if name.is_empty()
        || !name.is_ascii()
        || name.bytes().any(|b| {
            b.is_ascii_control()
                || matches!(b, b'\\' | b':' | b'<' | b'>' | b'"' | b'|' | b'?' | b'*')
        })
    {
        return Err(ArchiveError::UnsafeEntryName);
    }
    let directory = name.ends_with('/');
    let components = name.trim_end_matches('/').split('/');
    let count = components.clone().count();
    bounded(count as u64, limits.max_path_components)?;
    let mut parent = 0;
    for (index, part) in components.enumerate() {
        if part.is_empty() || matches!(part, "." | "..") || part.ends_with(['.', ' ']) {
            return Err(ArchiveError::UnsafeEntryName);
        }
        let last = index + 1 == count;
        let is_directory = !last || directory;
        let id = paths.len() + 1;
        match paths.entry((parent, AsciiComponent(part))) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let node = entry.get();
                if entry.key().1.0 != part
                    || node.directory != is_directory
                    || (last && node.explicit)
                {
                    return Err(ArchiveError::UnsafeEntryName);
                }
                parent = node.id;
                entry.get_mut().explicit |= last;
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(PathNode {
                    id,
                    directory: is_directory,
                    explicit: last,
                });
                parent = id;
            }
        }
    }
    if name.ends_with("//") {
        return Err(ArchiveError::UnsafeEntryName);
    }
    Ok(name.to_owned())
}

impl<'a, R: Read + Seek> ArchiveInventory<'a, R> {
    pub fn inspect(
        mut reader: R,
        limits: ArchiveLimits,
        cancel: &'a dyn Cancellation,
    ) -> Result<Self, ArchiveError> {
        limits.validate()?;
        cancelled(cancel)?;
        let length = reader
            .seek(SeekFrom::End(0))
            .map_err(|_| ArchiveError::ReadFailure)?;
        bounded(length, limits.max_archive_bytes)?;
        if length < 22 {
            return Err(ArchiveError::InvalidArchive);
        }
        let tail_length = length.min(65_557) as usize;
        let tail_start = length - tail_length as u64;
        let tail = read_at(&mut reader, tail_start, tail_length, cancel)?;
        let position = (0..=tail.len() - 22)
            .rev()
            .find(|&at| {
                tail[at..at + 4] == *b"PK\x05\x06"
                    && at + 22 + usize::from(u16_at(&tail, at + 20)) == tail.len()
            })
            .ok_or(ArchiveError::InvalidArchive)?;
        let end = &tail[position..];
        let directory_end = add(tail_start, position as u64)?;
        if u16_at(end, 8) == u16::MAX
            || u16_at(end, 10) == u16::MAX
            || u32_at(end, 12) == u32::MAX
            || u32_at(end, 16) == u32::MAX
        {
            return Err(ArchiveError::UnsupportedZip64);
        }
        if directory_end >= 20
            && read_at(&mut reader, directory_end - 20, 4, cancel)? == b"PK\x06\x07"
        {
            return Err(ArchiveError::UnsupportedZip64);
        }
        if u16_at(end, 4) != 0 || u16_at(end, 6) != 0 || u16_at(end, 8) != u16_at(end, 10) {
            return Err(ArchiveError::UnsupportedFeature);
        }
        let count = u64::from(u16_at(end, 10));
        let directory_size = u64::from(u32_at(end, 12));
        let directory_start = u64::from(u32_at(end, 16));
        if add(directory_start, directory_size)? != directory_end {
            return Err(ArchiveError::InvalidArchive);
        }
        bounded(count, limits.max_entries)?;
        bounded(directory_size, limits.max_directory_bytes)?;
        if count.checked_mul(46).ok_or(ArchiveError::LimitExceeded)? > directory_size {
            return Err(ArchiveError::InvalidArchive);
        }
        let mut directory = read_at(
            &mut reader,
            directory_start,
            directory_size as usize,
            cancel,
        )?;
        // The dependency retries earlier end records on metadata errors. There must
        // be no alternate end/ZIP64 candidates anywhere in the cached directory.
        if directory
            .windows(4)
            .any(|part| matches!(part, b"PK\x05\x06" | b"PK\x06\x06" | b"PK\x06\x07"))
        {
            return Err(ArchiveError::InvalidArchive);
        }
        let mut offset = 0usize;
        let mut entries = Vec::with_capacity(count as usize);
        let mut paths = BTreeMap::new();
        let mut ranges = Vec::with_capacity(count as usize);
        let mut total = 0;
        for _ in 0..count {
            cancelled(cancel)?;
            let header = directory
                .get(offset..offset + 46)
                .ok_or(ArchiveError::InvalidArchive)?;
            if header[..4] != *b"PK\x01\x02" {
                return Err(ArchiveError::InvalidArchive);
            }
            let compressed = u64::from(u32_at(header, 20));
            let expanded = u64::from(u32_at(header, 24));
            let local_offset = u64::from(u32_at(header, 42));
            if [compressed, expanded, local_offset].contains(&u64::from(u32::MAX))
                || u16_at(header, 34) == u16::MAX
            {
                return Err(ArchiveError::UnsupportedZip64);
            }
            let name_length = usize::from(u16_at(header, 28));
            let extra_length = usize::from(u16_at(header, 30));
            let comment_length = usize::from(u16_at(header, 32));
            let name_end = offset + 46 + name_length;
            let next = name_end + extra_length + comment_length;
            let raw = directory
                .get(offset + 46..name_end)
                .ok_or(ArchiveError::InvalidArchive)?;
            let name = validate_name(raw, limits, &mut paths)?;
            extra_fields(
                directory
                    .get(name_end..name_end + extra_length)
                    .ok_or(ArchiveError::InvalidArchive)?,
            )?;
            if next > directory.len() {
                return Err(ArchiveError::InvalidArchive);
            }
            let method = u16_at(header, 10);
            let flags = u16_at(header, 8);
            // Deflate option bits and UTF-8 are the only accepted flags. Data descriptors
            // are not accepted until their ambiguous variants have a reviewed policy.
            if flags & !0x0806 != 0 || !matches!(method, 0 | 8) || u16_at(header, 34) != 0 {
                return Err(ArchiveError::UnsupportedFeature);
            }
            let host = header[5];
            let attributes = u32_at(header, 38);
            let mode = (attributes >> 16) & 0o170000;
            let is_directory = name.ends_with('/');
            if !matches!(host, 0 | 3)
                || (host == 3
                    && mode != 0
                    && mode != if is_directory { 0o040000 } else { 0o100000 })
                || attributes & 8 != 0
                || (attributes & 0x10 != 0 && !is_directory)
                || (is_directory && (expanded != 0 || compressed != 0))
            {
                return Err(ArchiveError::UnsupportedFeature);
            }
            bounded(expanded, limits.max_entry_bytes)?;
            total = add(total, expanded)?;
            bounded(total, limits.max_total_declared_bytes)?;
            ratio(expanded, compressed, limits.max_expansion_ratio)?;
            if add(local_offset, 30)? > directory_start {
                return Err(ArchiveError::InvalidArchive);
            }
            let local = read_at(&mut reader, local_offset, 30, cancel)?;
            if local[..4] != *b"PK\x03\x04"
                || u16_at(&local, 4) != u16_at(header, 6)
                || local[10..14] != header[12..16]
                || u16_at(&local, 6) != flags
                || u16_at(&local, 8) != method
                || u32_at(&local, 14) != u32_at(header, 16)
                || u32_at(&local, 18) != compressed as u32
                || u32_at(&local, 22) != expanded as u32
                || usize::from(u16_at(&local, 26)) != name_length
            {
                return Err(ArchiveError::InvalidArchive);
            }
            let local_extra = usize::from(u16_at(&local, 28));
            let data_start = add(add(local_offset, 30)?, (name_length + local_extra) as u64)?;
            let data_end = add(data_start, compressed)?;
            if data_end > directory_start {
                return Err(ArchiveError::InvalidArchive);
            }
            let local_name_extra = read_at(
                &mut reader,
                local_offset + 30,
                name_length + local_extra,
                cancel,
            )?;
            if &local_name_extra[..name_length] != raw {
                return Err(ArchiveError::InvalidArchive);
            }
            extra_fields(&local_name_extra[name_length..])?;
            ranges.push((local_offset, data_end));
            entries.push(Entry {
                name,
                expanded,
                compressed,
                directory: is_directory,
                validated: false,
            });
            offset = next;
        }
        // Release the borrowed component index before constructing ZIP's index.
        drop(paths);
        if offset != directory.len() {
            return Err(ArchiveError::InvalidArchive);
        }
        ranges.sort_unstable();
        let mut previous_end = 0;
        for (start, end) in ranges {
            // Disallow prefixes, gaps, descriptors and unindexed local records.
            if start != previous_end {
                return Err(ArchiveError::InvalidArchive);
            }
            previous_end = end;
        }
        if previous_end != directory_start {
            return Err(ArchiveError::InvalidArchive);
        }
        cancelled(cancel)?;
        // During construction expose only the checked directory and one canonical
        // end record. Prefix bytes are virtual zeros; archive comments are omitted.
        // This also prevents rereading mutable metadata between preflight and indexing.
        directory.extend_from_slice(&end[..20]);
        directory.extend_from_slice(&[0, 0]);
        let indexing = Arc::new(AtomicBool::new(true));
        let reader_cancel = Arc::new(OnceLock::new());
        let reader = InventoryReader {
            inner: reader,
            metadata: directory,
            metadata_start: directory_start,
            position: 0,
            indexing: Arc::clone(&indexing),
            cancel,
            reader_cancel: Arc::clone(&reader_cancel),
        };
        let archive = ZipArchive::new(reader).map_err(|_| {
            if cancel.is_cancelled() {
                ArchiveError::Cancelled
            } else {
                ArchiveError::InvalidArchive
            }
        })?;
        indexing.store(false, Ordering::Relaxed);
        cancelled(cancel)?;
        if archive.len() != entries.len() {
            return Err(ArchiveError::InvalidArchive);
        }
        Ok(Self {
            archive,
            entries,
            limits,
            cancel,
            observed: 0,
            reader_cancel,
        })
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Bound to this retained inventory's lifetime and set once. The original
    /// token remains authoritative; a typed reader cannot replace either token.
    pub(crate) fn bind_reader_cancellation(
        &mut self,
        cancel: &'a dyn Cancellation,
    ) -> Result<(), ArchiveError> {
        self.reader_cancel
            .set(cancel)
            .map_err(|_| ArchiveError::InvalidSelection)
    }

    /// Selection is by generic extension only; it makes no schema claim.
    pub fn json_entries(&self) -> Vec<EntryIndex> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.directory && entry.name.ends_with(".json"))
            .map(|(index, _)| EntryIndex(index))
            .collect()
    }

    pub(crate) fn selected_name(&self, index: EntryIndex) -> Result<&str, ArchiveError> {
        self.entries
            .get(index.0)
            .filter(|entry| !entry.directory)
            .map(|entry| entry.name.as_str())
            .ok_or(ArchiveError::InvalidSelection)
    }

    pub fn is_validated(&self, index: EntryIndex) -> bool {
        self.entries
            .get(index.0)
            .is_some_and(|entry| entry.validated)
    }

    pub fn validate_entry(&mut self, index: EntryIndex) -> Result<(), ArchiveError> {
        self.consume(index, |_| Ok(()))
    }

    pub(crate) fn consume<T>(
        &mut self,
        index: EntryIndex,
        parse: impl FnOnce(&mut dyn Read) -> Result<T, ArchiveError>,
    ) -> Result<T, ArchiveError> {
        let entry = self
            .entries
            .get_mut(index.0)
            .ok_or(ArchiveError::InvalidSelection)?;
        entry.validated = false;
        cancelled(self.cancel)?;
        let file = self
            .archive
            .by_index(index.0)
            .map_err(|error| match error {
                zip::result::ZipError::Io(error) => entry_io_error(error),
                _ => ArchiveError::IntegrityFailure,
            })?;
        let mut reader = ObservedReader {
            inner: file,
            cancel: self.cancel,
            limits: self.limits,
            declared: entry.expanded,
            compressed: entry.compressed,
            seen: 0,
            total: &mut self.observed,
            failure: None,
        };
        let result = parse(&mut reader);
        if let Some(error) = reader.failure {
            return Err(error);
        }
        let result = result?;
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => (),
                Err(_) => return Err(reader.failure.unwrap_or(ArchiveError::IntegrityFailure)),
            }
        }
        if reader.seen != entry.expanded {
            return Err(ArchiveError::IntegrityFailure);
        }
        cancelled(self.cancel)?;
        entry.validated = true;
        Ok(result)
    }
}

struct InventoryReader<'a, R> {
    inner: R,
    metadata: Vec<u8>,
    metadata_start: u64,
    position: u64,
    indexing: Arc<AtomicBool>,
    cancel: &'a dyn Cancellation,
    reader_cancel: Arc<OnceLock<&'a dyn Cancellation>>,
}

impl<R: Read> Read for InventoryReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        cancelled(self.cancel).map_err(io::Error::other)?;
        if let Some(cancel) = self.reader_cancel.get() {
            cancelled(*cancel).map_err(io::Error::other)?;
        }
        if !self.indexing.load(Ordering::Relaxed) {
            let length = output.len().min(8192);
            return self.inner.read(&mut output[..length]);
        }
        let end = self.metadata_start + self.metadata.len() as u64;
        let count = output
            .len()
            .min(8192)
            .min(end.saturating_sub(self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        output[..count].fill(0);
        let finish = self.position + count as u64;
        if finish > self.metadata_start {
            let begin = self.position.max(self.metadata_start);
            let out_start = (begin - self.position) as usize;
            let in_start = (begin - self.metadata_start) as usize;
            output[out_start..count]
                .copy_from_slice(&self.metadata[in_start..in_start + count - out_start]);
        }
        self.position = finish;
        Ok(count)
    }
}

impl<R: Seek> Seek for InventoryReader<'_, R> {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        cancelled(self.cancel).map_err(io::Error::other)?;
        if let Some(cancel) = self.reader_cancel.get() {
            cancelled(*cancel).map_err(io::Error::other)?;
        }
        if !self.indexing.load(Ordering::Relaxed) {
            return self.inner.seek(offset);
        }
        let position = match offset {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::Current(value) => self.position.checked_add_signed(value),
            SeekFrom::End(value) => {
                (self.metadata_start + self.metadata.len() as u64).checked_add_signed(value)
            }
        }
        .ok_or_else(|| io::Error::other(ArchiveError::InvalidArchive))?;
        self.position = position;
        Ok(position)
    }
}

struct ObservedReader<'a, R> {
    inner: R,
    cancel: &'a dyn Cancellation,
    limits: ArchiveLimits,
    declared: u64,
    compressed: u64,
    seen: u64,
    total: &'a mut u64,
    failure: Option<ArchiveError>,
}

impl<R: Read> Read for ObservedReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let result = (|| {
            cancelled(self.cancel)?;
            let size = buffer.len().min(8192);
            let read = self
                .inner
                .read(&mut buffer[..size])
                .map_err(entry_io_error)?;
            self.seen = add(self.seen, read as u64)?;
            *self.total = add(*self.total, read as u64)?;
            bounded(self.seen, self.limits.max_entry_bytes)?;
            bounded(*self.total, self.limits.max_observed_bytes)?;
            ratio(self.seen, self.compressed, self.limits.max_expansion_ratio)?;
            if self.seen > self.declared {
                return Err(ArchiveError::IntegrityFailure);
            }
            Ok(read)
        })();
        result.map_err(|error| {
            self.failure = Some(error);
            io::Error::other(error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct NeverCancel;
    impl Cancellation for NeverCancel {
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn metadata_view_eof_and_seek_overflow_fail_without_panics() {
        let mut reader = InventoryReader {
            inner: Cursor::new(Vec::<u8>::new()),
            metadata: vec![0; 22],
            metadata_start: 0,
            position: 0,
            indexing: Arc::new(AtomicBool::new(true)),
            cancel: &NeverCancel,
            reader_cancel: Arc::new(OnceLock::new()),
        };
        reader.seek(SeekFrom::End(1)).unwrap();
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
        reader.seek(SeekFrom::Start(u64::MAX)).unwrap();
        assert!(reader.seek(SeekFrom::Current(1)).is_err());
    }

    #[test]
    fn reader_cancellation_binding_is_once_only_and_preserves_both_authorities() {
        struct Flag(AtomicBool);
        impl Cancellation for Flag {
            fn is_cancelled(&self) -> bool {
                self.0.load(Ordering::Relaxed)
            }
        }
        for original_cancels in [false, true] {
            let original = Flag(AtomicBool::new(false));
            let reader = Flag(AtomicBool::new(false));
            let mut empty_zip = vec![0; 22];
            empty_zip[..4].copy_from_slice(b"PK\x05\x06");
            let mut archive = ArchiveInventory::inspect(
                Cursor::new(empty_zip),
                ArchiveLimits::default(),
                &original,
            )
            .unwrap();
            archive.bind_reader_cancellation(&reader).unwrap();
            assert_eq!(
                archive.bind_reader_cancellation(&reader),
                Err(ArchiveError::InvalidSelection)
            );
            assert_eq!(
                archive.bind_reader_cancellation(&NeverCancel),
                Err(ArchiveError::InvalidSelection)
            );
            if original_cancels {
                original.0.store(true, Ordering::Relaxed)
            } else {
                reader.0.store(true, Ordering::Relaxed)
            };
            assert_eq!(
                archive.bind_reader_cancellation(&NeverCancel),
                Err(ArchiveError::InvalidSelection)
            );
            let mut input = archive.archive.into_inner();
            // The retained input cannot keep working under either cancelled
            // authority, even if a replacement token would claim not cancelled.
            assert_eq!(
                entry_io_error(input.read(&mut [0; 1]).unwrap_err()),
                ArchiveError::Cancelled
            );
            assert_eq!(
                entry_io_error(input.seek(SeekFrom::Start(0)).unwrap_err()),
                ArchiveError::Cancelled
            );
        }
    }
}
