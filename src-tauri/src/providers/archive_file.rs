use std::fs::File;
#[cfg(unix)]
pub(crate) fn open_selected(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "selected archive is not a regular file",
        ));
    }
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}
#[cfg(not(unix))]
pub(crate) fn open_selected(path: &std::path::Path) -> std::io::Result<File> {
    File::open(path)
}

#[derive(PartialEq, Eq)]
pub(super) struct FileIdentity {
    dev: u64,
    inode: u64,
    links: u64,
    pub(super) size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl FileIdentity {
    pub(super) fn read(file: &File) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata().ok()?;
        if !m.is_file() || m.nlink() == 0 {
            return None;
        }
        Some(Self {
            dev: m.dev(),
            inode: m.ino(),
            links: m.nlink(),
            size: m.size(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
    pub(super) fn verify(&self, file: &File) -> Option<()> {
        (Self::read(file).as_ref() == Some(self)).then_some(())
    }
}

#[cfg(test)]
#[path = "../../../crates/discord-archive/tests/common/mod.rs"]
pub(crate) mod zip_fixture;
