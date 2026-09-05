# Archive storage

Retract contains the native dependency foundation for a future encrypted archive index. No real archive importer, archive schema, background indexing service, or new destructive command is enabled at this stage. Telegram continues to use its existing live-provider and foundation-store paths; live Telegram message bodies and existing jobs are not copied into this database.

## Native dependency

The archive connection gate uses `rusqlite` 0.40.2 with default features disabled and only `bundled-sqlcipher-vendored-openssl` enabled. The resolved native binding is `libsqlite3-sys` 0.38.2. Its bundled amalgamation identifies SQLCipher Community 4.14.0 over SQLite 3.51.3, and the crypto provider is the statically built OpenSSL 3.6.3 source supplied by `openssl-src` 300.6.1+3.6.3.

Exact Cargo package checksums, crate VCS revisions, amalgamation hashes, compiler defines, OpenSSL Configure arguments, and target mappings are recorded in [`vendor/sqlcipher/provenance.json`](../vendor/sqlcipher/provenance.json). The applicable SQLCipher and OpenSSL notices are shipped in the macOS application under `Contents/Resources/licenses/`.

## Connection policy

Archive keys are independent 32-byte backend secrets held in zeroizing memory. They are not serializable or formatted into diagnostics. A connection applies its key before reading the schema, verifies that the SQLCipher provider is OpenSSL, performs an authenticated schema read, and verifies FTS5 before it is returned. File-backed temporary stores, extension loading, SQLCipher logging, disabled foreign keys, relaxed synchronous writes, or disabled secure-delete cause the gate to fail.

Existing symlinks, nonregular files, plaintext databases, wrong-key databases, and tampered stores are rejected without replacement. Opening a missing path with creation disabled does not create a file. Public errors are fixed content-free codes and do not expose paths, SQL, keys, or native database messages.

SQLCipher protects database and rollback/WAL pages, but encryption alone is not a forensic-erasure guarantee. SSD behavior, filesystem snapshots, backups, and original archive exports can retain data. Import limits, schema migration, indexed queries, source removal, maintenance recovery, and OS-vault integration belong to later reviewed tasks.

## Verification gates

The focused codec suite creates a real encrypted database, enables FTS5 secure-delete, reopens it, searches and deletes a literal record, and rejects a wrong key. Separate real-file tests cover plaintext preservation, page tampering, missing-codec policy, file-backed temporary-store rejection, disabled extension loading, fixed error codes, and plaintext-canary absence from the database, WAL, and rollback journal.

Linux checks run offline and non-root in the existing Docker build on both arm64 and amd64 with reusable named caches. The required Apple-silicon macOS workflow runs the focused codec tests before unsigned packaging, checks the packaged executable and TDLib runtime dependencies, and rejects developer-machine library paths. Linux success is not evidence of macOS linkage: schema work remains blocked until the native macOS test and package job is green for the exact reviewed commit.
