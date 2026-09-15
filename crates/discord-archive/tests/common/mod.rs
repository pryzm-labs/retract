#![allow(dead_code)]

use discord_archive::{ArchiveLimits, Cancellation};

pub struct NeverCancel;
impl Cancellation for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

pub fn limits() -> ArchiveLimits {
    ArchiveLimits {
        max_archive_bytes: 100_000,
        max_directory_bytes: 10_000,
        max_entries: 20,
        max_path_bytes: 200,
        max_path_components: 10,
        max_entry_bytes: 20_000,
        max_total_declared_bytes: 40_000,
        max_observed_bytes: 30_000,
        max_expansion_ratio: 100,
        max_json_depth: 12,
        max_scalar_bytes: 1000,
        max_raw_record_bytes: 8000,
        max_decoded_record_bytes: 2000,
        max_json_tokens: 1000,
        max_display_bytes: 1000,
        max_selected_contexts: 20,
        max_structure_bytes: 10_000,
    }
}

pub fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
pub fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
pub fn signature(bytes: &[u8], value: &[u8; 4]) -> usize {
    bytes.windows(4).position(|p| p == value).unwrap()
}
pub fn central(bytes: &[u8]) -> usize {
    signature(bytes, b"PK\x01\x02")
}
pub fn eocd(bytes: &[u8]) -> usize {
    signature(bytes, b"PK\x05\x06")
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut directory = Vec::new();
    for (name, payload) in entries {
        let offset = bytes.len() as u32;
        let mut local = vec![0; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        put16(&mut local, 4, 20);
        put32(&mut local, 14, crc32(payload));
        put32(&mut local, 18, payload.len() as u32);
        put32(&mut local, 22, payload.len() as u32);
        put16(&mut local, 26, name.len() as u16);
        bytes.extend(local);
        bytes.extend(name.as_bytes());
        bytes.extend(*payload);
        let mut entry = vec![0; 46];
        entry[..4].copy_from_slice(b"PK\x01\x02");
        put16(&mut entry, 4, 0x0314);
        put16(&mut entry, 6, 20);
        put32(&mut entry, 16, crc32(payload));
        put32(&mut entry, 20, payload.len() as u32);
        put32(&mut entry, 24, payload.len() as u32);
        put16(&mut entry, 28, name.len() as u16);
        let mode = if name.ends_with('/') {
            0o040755
        } else {
            0o100644
        };
        put32(&mut entry, 38, mode << 16);
        put32(&mut entry, 42, offset);
        directory.extend(entry);
        directory.extend(name.as_bytes());
    }
    let offset = bytes.len();
    let size = directory.len();
    bytes.extend(directory);
    let mut end = vec![0; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    put16(&mut end, 8, entries.len() as u16);
    put16(&mut end, 10, entries.len() as u16);
    put32(&mut end, 12, size as u32);
    put32(&mut end, 16, offset as u32);
    bytes.extend(end);
    bytes
}
