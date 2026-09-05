//! One fail-fast credential owner for every profile sharing the macOS item.
use crate::error::AppError;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::ErrorKind,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) struct CredentialLease(File);

fn invalid() -> AppError {
    AppError::SecureStore("credential ownership file is invalid".into())
}

fn user_id() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // Supported Unix platforms expose geteuid as a no-argument, non-mutating call.
    unsafe { geteuid() }
}

fn validate_directory(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute() || fs::canonicalize(path).map_err(|_| invalid())? != path {
        return Err(invalid());
    }
    let directory = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !directory.is_dir() || directory.uid() != user_id() || directory.mode() & 0o022 != 0 {
        return Err(invalid());
    }
    Ok(())
}

fn validate_file(metadata: &fs::Metadata) -> Result<(), AppError> {
    if metadata.is_file()
        && metadata.nlink() == 1
        && metadata.uid() == user_id()
        && metadata.mode() & 0o077 == 0
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

impl CredentialLease {
    pub(super) fn acquire(root: &Path) -> Result<Self, AppError> {
        if let Err(error) = fs::symlink_metadata(root) {
            if error.kind() != ErrorKind::NotFound {
                return Err(invalid());
            }
            validate_directory(root.parent().ok_or_else(invalid)?)?;
            match fs::DirBuilder::new().mode(0o700).create(root) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return Err(invalid()),
            }
        }
        validate_directory(root)?;
        let path = root.join("credentials.lock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) => validate_file(&metadata)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(invalid()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).mode(0o600);
        #[cfg(target_os = "linux")]
        options.custom_flags(0x20000);
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100);
        let file = options.open(&path).map_err(|_| invalid())?;
        let opened = file.metadata().map_err(|_| invalid())?;
        validate_file(&opened)?;
        let named = fs::symlink_metadata(&path).map_err(|_| invalid())?;
        validate_file(&named)?;
        if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
            return Err(invalid());
        }
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(TryLockError::WouldBlock) => Err(AppError::ProfileInUse),
            Err(TryLockError::Error(_)) => Err(invalid()),
        }
    }
}

impl Drop for CredentialLease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
