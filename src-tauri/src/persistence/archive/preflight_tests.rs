use std::{
    fs,
    io::{self, Write},
    os::unix::fs::MetadataExt,
};

use super::*;
use crate::persistence::archive::{
    schema,
    test_support::{Fixture, key, snapshot_recovery_files},
};

fn staged_fixture() -> Fixture {
    let fixture = Fixture::new();
    drop(fixture.open());
    // A harmless zeroed rollback journal still exercises private copy handling.
    let journal = sidecar(&fixture.path, "-journal");
    let mut file = private_options().create_new(true).open(journal).unwrap();
    file.write_all(&[0; 512]).unwrap();
    fixture
}

fn staging_directories(fixture: &Fixture) -> Vec<PathBuf> {
    fs::read_dir(fixture.path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".archive-preflight-")
        })
        .collect()
}

#[test]
fn staged_files_are_private_ciphertext_and_only_current_stage_is_cleaned() {
    let fixture = staged_fixture();
    let previous = fixture
        .path
        .parent()
        .unwrap()
        .join(".archive-preflight-interrupted");
    DirBuilder::new().mode(0o700).create(&previous).unwrap();
    fs::write(
        previous.join("archive.db"),
        b"synthetic interrupted ciphertext",
    )
    .unwrap();
    let before = snapshot_recovery_files(&fixture.path);
    validate_existing(&fixture.path, &key(), |db| {
        schema::validate(db)?;
        let stage = staging_directories(&fixture)
            .into_iter()
            .find(|path| path != &previous)
            .unwrap();
        assert_eq!(fs::metadata(&stage).unwrap().mode() & 0o777, 0o700);
        let bytes = fs::read(stage.join("archive.db")).unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3"));
        for entry in fs::read_dir(stage).unwrap() {
            assert_eq!(entry.unwrap().metadata().unwrap().mode() & 0o777, 0o600);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        staging_directories(&fixture),
        std::slice::from_ref(&previous)
    );
    assert_eq!(
        fs::read(previous.join("archive.db")).unwrap(),
        b"synthetic interrupted ciphertext"
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn partial_stage_copy_and_validation_failure_preserve_originals_and_clean_owned_files() {
    struct FullStage<'a> {
        file: &'a mut File,
        remaining: usize,
    }
    impl Write for FullStage<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::from_raw_os_error(28));
            }
            let written = self.file.write(&bytes[..bytes.len().min(self.remaining)])?;
            self.remaining -= written;
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }
    let fixture = staged_fixture();
    let before = snapshot_recovery_files(&fixture.path);
    let result = validate_with_copy(
        &fixture.path,
        &key(),
        |_| panic!("failed copy reached validation"),
        |source, destination, length| {
            let mut full_stage = FullStage {
                file: destination,
                remaining: 4096,
            };
            let error = copy_stream(source, &mut full_stage, length).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(28));
            Err(ArchiveError::StorageFailure)
        },
    );
    assert_eq!(result, Err(ArchiveError::StorageFailure));
    assert!(staging_directories(&fixture).is_empty());
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    assert_eq!(
        validate_existing(&fixture.path, &key(), |_| Err(ArchiveError::InvalidRecord)),
        Err(ArchiveError::InvalidRecord)
    );
    assert!(staging_directories(&fixture).is_empty());
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
}

#[test]
fn streaming_copy_propagates_disk_full_after_partial_write() {
    struct FullDisk(Vec<u8>);
    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len() == 17 {
                return Err(io::Error::from_raw_os_error(28));
            }
            let length = bytes.len().min(17 - self.0.len());
            self.0.extend_from_slice(&bytes[..length]);
            Ok(length)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut destination = FullDisk(Vec::new());
    assert_eq!(
        copy_stream(&mut &[0_u8; 100][..], &mut destination, 100)
            .unwrap_err()
            .raw_os_error(),
        Some(28)
    );
    assert_eq!(destination.0.len(), 17);
}

#[test]
fn observable_original_changes_during_staged_validation_are_rejected() {
    let fixture = staged_fixture();
    assert_eq!(
        validate_existing(&fixture.path, &key(), |_| {
            private_options()
                .append(true)
                .open(&fixture.path)
                .unwrap()
                .write_all(b"external change")
                .unwrap();
            Ok(())
        }),
        Err(ArchiveError::InvalidStore)
    );
    assert!(
        fs::read(&fixture.path)
            .unwrap()
            .ends_with(b"external change")
    );
    assert!(staging_directories(&fixture).is_empty());
}

#[test]
fn journal_with_super_journal_trailer_is_rejected_without_following_its_reference() {
    let fixture = staged_fixture();
    let journal = sidecar(&fixture.path, "-journal");
    let mut file = private_options().append(true).open(journal).unwrap();
    file.write_all(b"unrelated-master").unwrap();
    file.write_all(&16_u32.to_be_bytes()).unwrap();
    file.write_all(&0_u32.to_be_bytes()).unwrap();
    file.write_all(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7])
        .unwrap();
    drop(file);
    let before = snapshot_recovery_files(&fixture.path);
    assert_eq!(
        validate_existing(&fixture.path, &key(), |_| Ok(())),
        Err(ArchiveError::InvalidStore)
    );
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    assert!(staging_directories(&fixture).is_empty());
}
