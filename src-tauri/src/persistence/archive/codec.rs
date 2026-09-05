use std::{fmt::Write as _, fs, io::ErrorKind, path::Path};

use rusqlite::{Connection, OpenFlags, ffi};
use zeroize::Zeroizing;

use super::ArchiveError;

pub(crate) struct ArchiveKey(Zeroizing<[u8; 32]>);

impl ArchiveKey {
    pub(crate) fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    fn sqlcipher_literal(&self) -> Zeroizing<String> {
        let mut literal = Zeroizing::new(String::with_capacity(67));
        literal.push_str("x'");
        for byte in self.0.iter() {
            write!(&mut literal, "{byte:02x}").expect("writing into a String cannot fail");
        }
        literal.push('\'');
        literal
    }
}

pub(super) fn open_keyed(
    path: &Path,
    key: &ArchiveKey,
    create: bool,
) -> Result<Connection, ArchiveError> {
    validate_path(path, create)?;

    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | OpenFlags::SQLITE_OPEN_EXRESCODE;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }

    let connection = Connection::open_with_flags(path, flags).map_err(|_| {
        if create {
            ArchiveError::StorageFailure
        } else {
            ArchiveError::InvalidStore
        }
    })?;

    connection
        .pragma_update(None, "cipher_log_level", "NONE")
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    let key_literal = key.sqlcipher_literal();
    connection
        .pragma_update(None, "key", key_literal.as_str())
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    drop(key_literal);

    validate_codec(&connection)?;
    authenticate_schema(&connection)?;
    configure_connection(&connection)?;
    validate_fts(&connection)?;

    Ok(connection)
}

fn validate_path(path: &Path, create: bool) -> Result<(), ArchiveError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ArchiveError::InvalidStore)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound && create => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(ArchiveError::InvalidStore),
        Err(_) => Err(ArchiveError::StorageFailure),
    }
}

fn validate_codec(connection: &Connection) -> Result<(), ArchiveError> {
    let version = pragma_string(connection, "cipher_version")?;
    let provider = pragma_string(connection, "cipher_provider")?;
    validate_codec_metadata(version.as_deref(), provider.as_deref())
}

fn pragma_string(connection: &Connection, name: &str) -> Result<Option<String>, ArchiveError> {
    let mut value = None;
    connection
        .pragma_query(None, name, |row| {
            value = Some(row.get(0)?);
            Ok(())
        })
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    Ok(value)
}

pub(super) fn validate_codec_metadata(
    version: Option<&str>,
    provider: Option<&str>,
) -> Result<(), ArchiveError> {
    match (version, provider) {
        (Some("4.14.0 community"), Some("openssl")) => Ok(()),
        _ => Err(ArchiveError::UnsupportedCodec),
    }
}

fn authenticate_schema(connection: &Connection) -> Result<(), ArchiveError> {
    connection
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|_| ())
        .map_err(|_| ArchiveError::InvalidStore)
}

fn configure_connection(connection: &Connection) -> Result<(), ArchiveError> {
    disable_extension_loading(connection)?;
    connection
        .execute_batch(
            "PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             PRAGMA secure_delete = ON;
             PRAGMA trusted_schema = OFF;",
        )
        .map_err(|_| ArchiveError::StorageFailure)?;

    validate_connection_settings(connection)
}

pub(super) fn validate_connection_settings(connection: &Connection) -> Result<(), ArchiveError> {
    let temp_store: i64 = connection
        .pragma_query_value(None, "temp_store", |row| row.get(0))
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    let foreign_keys: i64 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    let secure_delete: i64 = connection
        .pragma_query_value(None, "secure_delete", |row| row.get(0))
        .map_err(|_| ArchiveError::UnsupportedCodec)?;
    if temp_store != 2 || foreign_keys != 1 || synchronous != 2 || secure_delete != 1 {
        return Err(ArchiveError::UnsupportedCodec);
    }
    Ok(())
}

fn disable_extension_loading(connection: &Connection) -> Result<(), ArchiveError> {
    // SAFETY: rusqlite owns this live handle for the duration of the call. Passing zero only
    // disables a connection-local capability and does not transfer or retain any pointer.
    let result = unsafe { ffi::sqlite3_enable_load_extension(connection.handle(), 0) };
    if result == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(ArchiveError::UnsupportedCodec)
    }
}

fn validate_fts(connection: &Connection) -> Result<(), ArchiveError> {
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE temp.archive_fts_capability USING fts5(body);
             DROP TABLE temp.archive_fts_capability;",
        )
        .map_err(|_| ArchiveError::UnsupportedCodec)
}
