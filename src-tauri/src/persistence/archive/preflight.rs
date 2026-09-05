//! Validate an existing archive without permitting recovery to alter rejected
//! originals. Staging is disposable ciphertext, never an alternate data source.

use std::{
    fs::{self, DirBuilder, File, Metadata},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use uuid::Uuid;

use super::{
    ArchiveError, ArchiveKey,
    codec::{open_immutable_keyed, open_keyed},
    store::{private_options, validate_parent},
};

const SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
}

impl Stamp {
    fn from_metadata(metadata: &Metadata) -> Result<Self, ArchiveError> {
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            return Err(ArchiveError::InvalidStore);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            mode: metadata.mode(),
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Snapshot([Option<Stamp>; 4]);

impl Snapshot {
    fn read(path: &Path) -> Result<Self, ArchiveError> {
        validate_parent(path)?;
        let mut stamps = [None, None, None, None];
        for (index, suffix) in SUFFIXES.iter().enumerate() {
            stamps[index] = match fs::symlink_metadata(sidecar(path, suffix)) {
                Ok(metadata) => Some(Stamp::from_metadata(&metadata)?),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(_) => return Err(ArchiveError::StorageFailure),
            };
        }
        let [database, wal, shm, journal] = &stamps;
        if (database.is_none() && (wal.is_some() || shm.is_some() || journal.is_some()))
            || (shm.is_some() && wal.is_none())
            || (journal.is_some() && (wal.is_some() || shm.is_some()))
        {
            return Err(ArchiveError::InvalidStore);
        }
        Ok(Self(stamps))
    }

    fn verify(&self, path: &Path) -> Result<(), ArchiveError> {
        if self != &Self::read(path)? {
            return Err(ArchiveError::InvalidStore);
        }
        Ok(())
    }

    fn needs_recovery(&self) -> bool {
        self.0.iter().skip(1).any(Option::is_some)
    }
}

pub(super) fn validate_artifact_set(path: &Path) -> Result<(), ArchiveError> {
    Snapshot::read(path).map(|_| ())
}

pub(super) fn validate_existing(
    path: &Path,
    key: &ArchiveKey,
    validate: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    validate_with_copy(path, key, validate, |source, destination, length| {
        copy_stream(source, destination, length).map_err(|_| ArchiveError::StorageFailure)
    })
}

fn validate_with_copy(
    path: &Path,
    key: &ArchiveKey,
    validate: impl FnOnce(&Connection) -> Result<(), ArchiveError>,
    mut copy: impl FnMut(&mut File, &mut File, u64) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let snapshot = Snapshot::read(path)?;
    if snapshot.0[0].as_ref().is_none_or(|stamp| stamp.length == 0) {
        return Err(ArchiveError::InvalidStore);
    }
    let stage = if snapshot.needs_recovery() {
        let stage = Stage::create(path.parent().ok_or(ArchiveError::InvalidStore)?)?;
        for (suffix, stamp) in SUFFIXES.iter().zip(snapshot.0.iter()) {
            // SHM is a derived WAL index with process-local lock state. Rebuild
            // it only inside staging; never change or remove the original SHM.
            if *suffix == "-shm" {
                continue;
            }
            if let Some(stamp) = stamp {
                let mut source = private_options()
                    .write(false)
                    .open(sidecar(path, suffix))
                    .map_err(|_| ArchiveError::InvalidStore)?;
                if Stamp::from_metadata(
                    &source
                        .metadata()
                        .map_err(|_| ArchiveError::StorageFailure)?,
                )? != *stamp
                {
                    return Err(ArchiveError::InvalidStore);
                }
                if *suffix == "-journal" {
                    reject_super_journal(&mut source, stamp.length)?;
                }
                let mut destination = private_options()
                    .create_new(true)
                    .open(sidecar(&stage.database(), suffix))
                    .map_err(|_| ArchiveError::StorageFailure)?;
                copy(&mut source, &mut destination, stamp.length)?;
                destination
                    .sync_all()
                    .map_err(|_| ArchiveError::StorageFailure)?;
                if Stamp::from_metadata(
                    &source
                        .metadata()
                        .map_err(|_| ArchiveError::StorageFailure)?,
                )? != *stamp
                {
                    return Err(ArchiveError::InvalidStore);
                }
            }
        }
        snapshot.verify(path)?;
        Some(stage)
    } else {
        None
    };
    let probe = match &stage {
        Some(stage) => open_keyed(&stage.database(), key, false)?,
        None => open_immutable_keyed(path, key)?,
    };
    let result = validate(&probe);
    drop(probe);
    snapshot.verify(path)?;
    if let Some(stage) = stage {
        stage.finish()?;
    }
    result
}

fn copy_stream(
    source: &mut impl Read,
    destination: &mut impl Write,
    length: u64,
) -> io::Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut remaining = length;
    while remaining != 0 {
        let capacity = remaining.min(buffer.len() as u64) as usize;
        let read = source.read(&mut buffer[..capacity])?;
        if read == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        destination.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    if source.read(&mut buffer[..1])? != 0 {
        return Err(io::Error::other("archive snapshot changed"));
    }
    Ok(())
}

fn reject_super_journal(file: &mut File, length: u64) -> Result<(), ArchiveError> {
    // Archive writes never ATTACH another database. SQLite's super-journal
    // trailer can name files outside staging, so that artifact is ambiguous
    // and must not reach recovery. Recognize only the fixed trailer marker;
    // never interpret or follow the embedded filename.
    if length >= 16 {
        file.seek(SeekFrom::End(-8))
            .map_err(|_| ArchiveError::StorageFailure)?;
        let mut trailer = [0; 8];
        file.read_exact(&mut trailer)
            .map_err(|_| ArchiveError::StorageFailure)?;
        if trailer == [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7] {
            return Err(ArchiveError::InvalidStore);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| ArchiveError::StorageFailure)?;
    }
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

struct Stage {
    directory: PathBuf,
    cleaned: bool,
}

impl Stage {
    fn create(parent: &Path) -> Result<Self, ArchiveError> {
        let directory = parent.join(format!(".archive-preflight-{}", Uuid::new_v4()));
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| ArchiveError::StorageFailure)?;
        Ok(Self {
            directory,
            cleaned: false,
        })
    }

    fn database(&self) -> PathBuf {
        self.directory.join("archive.db")
    }

    fn cleanup(&mut self) -> Result<(), ArchiveError> {
        if self.cleaned {
            return Ok(());
        }
        for suffix in SUFFIXES {
            match fs::remove_file(sidecar(&self.database(), suffix)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(_) => return Err(ArchiveError::StorageFailure),
            }
        }
        fs::remove_dir(&self.directory).map_err(|_| ArchiveError::StorageFailure)?;
        self.cleaned = true;
        Ok(())
    }

    fn finish(mut self) -> Result<(), ArchiveError> {
        self.cleanup()
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        // Delete only this exclusive operation's fixed artifact names. A crash
        // or cleanup I/O error may leave ciphertext, never a recovery fallback.
        let _ = self.cleanup();
    }
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;
