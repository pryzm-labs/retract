# Archive storage

Retract contains the native dependency foundation and an internal encrypted archive repository. No real archive importer, production archive service, background indexing, or new destructive command is enabled at this stage. Telegram continues to use its existing live-provider and foundation-store paths; live Telegram message bodies and existing jobs are not copied into this database.

## Native dependency

The archive connection gate uses `rusqlite` 0.40.2 with default features disabled and only `bundled-sqlcipher-vendored-openssl` enabled. The resolved native binding is `libsqlite3-sys` 0.38.2. Its bundled amalgamation identifies SQLCipher Community 4.14.0 over SQLite 3.51.3, and the crypto provider is the statically built OpenSSL 3.6.3 source supplied by `openssl-src` 300.6.1+3.6.3.

Exact Cargo package checksums, crate VCS revisions, amalgamation hashes, compiler defines, OpenSSL Configure arguments, and target mappings are recorded in [`vendor/sqlcipher/provenance.json`](../vendor/sqlcipher/provenance.json). The applicable SQLCipher and OpenSSL notices are shipped in the macOS application under `Contents/Resources/licenses/`.

## Connection policy

Archive keys are independent 32-byte backend secrets held in zeroizing memory. They are not serializable or formatted into diagnostics. A connection applies its key before reading the schema, verifies that the SQLCipher provider is OpenSSL, performs an authenticated schema read, and verifies FTS5 before it is returned. File-backed temporary stores, extension loading, SQLCipher logging, disabled foreign keys, relaxed synchronous writes, or disabled secure-delete cause the gate to fail.

Existing symlinks, nonregular files, plaintext databases, wrong-key databases, and tampered stores are rejected without replacement. Opening a missing path with creation disabled does not create a file. Public errors are fixed content-free codes and do not expose paths, SQL, keys, or native database messages.

SQLCipher protects database and rollback/WAL pages, but encryption alone is not a forensic-erasure guarantee. SSD behavior, filesystem snapshots, backups, and original archive exports can retain data. Import limits, schema migration, indexed queries, source removal, maintenance recovery, and OS-vault integration belong to later reviewed tasks.

## Repository and interrupted-file safety

The internal repository holds an exclusive process lock for its connection lifetime. Account identities are adapter-validated and provider-scoped; source observations remain separate from shared resource identities. New sources are incomplete until a later import finalizer publishes readiness. Registering another source does not implicitly replace stored account metadata.

Existing databases are validated before they are opened for writes. Clean databases use a keyed immutable probe. Databases with recovery files are first copied as ciphertext into a private, temporary staging directory, where recovery and full schema/registration validation occur. Only an accepted original is then opened for normal recovery. An unsupported original and its recovery files remain unchanged; the staged copy never replaces it or becomes a fallback.

Recovery preflight temporarily requires disk space for the database and its recovery files, plus possible recovery growth. Copies use bounded buffers, but this extra disk use is outside the normalized-import quota. A failed or interrupted cleanup can leave private encrypted staging files; they are never used to restore older content. This does not claim protection against a malicious same-user process that bypasses the application lock.

## Verification gates

The focused codec suite creates a real encrypted database, enables FTS5 secure-delete, reopens it, searches and deletes a literal record, and rejects a wrong key. Separate real-file tests cover plaintext preservation, page tampering, missing-codec policy, file-backed temporary-store rejection, disabled extension loading, fixed error codes, and plaintext-canary absence from the database, WAL, and rollback journal.

Linux checks run offline and non-root in the existing Docker build on both arm64 and amd64 with reusable named caches. The required Apple-silicon macOS workflow runs the focused codec tests before unsigned packaging, checks the packaged executable and TDLib runtime dependencies, and rejects developer-machine library paths. Linux success is not evidence of macOS linkage: schema work remains blocked until the native macOS test and package job is green for the exact reviewed commit.

The initial dependency gate passed on 2026-09-05 in [Secure build run 33979891484](https://github.com/pryzm-labs/retract/actions/runs/33979891484), at commit `cdcc083e8c604fba58b13ecb85e883276e3ec40a`. Both Linux architectures passed their checks; Apple silicon passed all 12 codec tests and the unsigned app's bundle, signature, architecture, checksum and runtime-dependency validation. This verifies the native dependency foundation, not archive functionality or real Keychain prompt behavior. The final integrated archive implementation must run the gates again.

The repository and recovery-preservation changes were independently reviewed through `6616087`. At that revision, Docker arm64 passed 45 focused archive tests, formatting and strict Clippy. These synthetic tests include rejected WAL/hot-journal byte preservation, supported crash recovery, private staging, partial-copy failure and retained-account validation. Native verification of these later changes remains part of the final integration gate.
