# Archive storage

Retract contains an internal encrypted archive repository and a lazy, bounded backend service. This is backend foundation only: no real archive importer, Discord/X support, source-switching UI, background indexing, or new destructive command is enabled. Telegram continues to use its existing live-provider and foundation-store paths; live Telegram message bodies and existing jobs are not copied into this database.

## Native dependency

The archive connection gate uses `rusqlite` 0.40.2 with default features disabled and only `bundled-sqlcipher-vendored-openssl` enabled. The resolved native binding is `libsqlite3-sys` 0.38.2. Its bundled amalgamation identifies SQLCipher Community 4.14.0 over SQLite 3.51.3, and the crypto provider is the statically built OpenSSL 3.6.3 source supplied by `openssl-src` 300.6.1+3.6.3.

Exact Cargo package checksums, crate VCS revisions, amalgamation hashes, compiler defines, OpenSSL Configure arguments, and target mappings are recorded in [`vendor/sqlcipher/provenance.json`](../vendor/sqlcipher/provenance.json). The applicable SQLCipher and OpenSSL notices are shipped in the macOS application under `Contents/Resources/licenses/`.

## Connection policy

Archive keys are independent 32-byte backend secrets held in zeroizing memory. They are not serializable or formatted into diagnostics. A connection applies its key before reading the schema, verifies that the SQLCipher provider is OpenSSL, performs an authenticated schema read, and verifies FTS5 before it is returned. File-backed temporary stores, extension loading, SQLCipher logging, disabled foreign keys, relaxed synchronous writes, or disabled secure-delete cause the gate to fail.

Existing symlinks, nonregular files, plaintext databases, wrong-key databases, and tampered stores are rejected without replacement. Opening a missing path with creation disabled does not create a file. Public errors are fixed content-free codes and do not expose paths, SQL, keys, or native database messages.

SQLCipher protects database and rollback/WAL pages, but encryption alone is not a forensic-erasure guarantee. SSD behavior, filesystem snapshots, backups, and original archive exports can retain data. The service, source removal, maintenance recovery and app-wide credential ownership are tested with synthetic data and injected credentials; real Keychain/privacy checks remain manual gates.

## Lazy ownership and credentials

Backend setup binds the macOS credential cache to the explicit application data root without creating files or reading secrets. All Telegram settings, profile keys and archive keys share one exclusive, fail-fast `credentials.lock` lease held for the cached-vault lifetime. A competing process/profile receives the existing safe profile-in-use error before vault I/O. Cached settings access can precede a profile/archive open; store locks are acquired before entering the synchronized vault, and no store callback or store-lock acquisition occurs while the vault mutex is held. An existing credential lease is reused.

The application owns every lazily opened archive worker. Only first use prepares the private `<app_local_data>/archives/content.db` location and requests the independent archive key, on a blocking worker after acquiring the archive process lock. Unsupported production targets return `UnavailableKey` without creating archive files. Input is bounded before queueing/copying; at most two batches wait behind the active operation. A full queue returns retryable `Busy`, while actual input limits return `LimitExceeded`.

A failed first open remains cached for the application lifetime to avoid repeated credential prompts and I/O. Correct the condition, then restart Retract. This includes initial store contention and differs from explicit import retry or backoff after transient queue `Busy`; an archive-open retry UI is future work.

Shutdown permanently prevents new archive work, cancels unstarted commands and drains each tracked repository before zeroizing cached secrets and releasing credential ownership. Cancelling an open or shutdown waiter does not lose the worker's completion handle. Cache clearing is terminal: a late settings request cannot reacquire the lease. Cancellation of an import sets its shared signal before waiting for a queue slot; queued batches and the in-flight pre-commit check observe it. Already committed progress is retained.

Quit **all older Retract copies before first archive use**. An already-running older binary cannot be made to honor the new lease. First archive-key creation lazily upgrades the same consolidated Keychain item from v1 to namespaced v2 while preserving Telegram secret values. Unknown versions are never overwritten; vault-format downgrades are unsupported, and deleting/restoring older vault bytes is not a recovery procedure. The archive key is never accepted through IPC, environment variables or a user-supplied production path.

## Repository and interrupted-file safety

The internal repository holds an exclusive process lock for its connection lifetime. Account identities are adapter-validated and provider-scoped; source observations remain separate from shared resource identities. New sources are incomplete until the import finalizer publishes readiness. Registering another source does not implicitly replace stored account metadata.

Existing databases are validated before they are opened for writes. Clean databases use a keyed immutable probe. Databases with recovery files are first copied as ciphertext into a private, temporary staging directory, where recovery and full schema/registration validation occur. Only an accepted original is then opened for normal recovery. An unsupported original and its recovery files remain unchanged; the staged copy never replaces it or becomes a fallback.

Recovery preflight temporarily requires disk space for the database and its recovery files, plus possible recovery growth. Copies use bounded buffers, but this extra disk use is outside the normalized-import quota. A failed or interrupted cleanup can leave private encrypted staging files; they are never used to restore older content. This does not claim protection against a malicious same-user process that bypasses the application lock.

## Internal import and query contracts

Imports use backend-issued sessions and explicitly numbered batches. Observations, FTS entries, findings, counters and replay receipts commit together. Exact replay of an accepted batch returns its original checkpoint without charging it twice. Reopening an interrupted import does not resume it automatically; explicit retry validates its persisted provenance and rotates its mutation authority. Incomplete sources cannot be searched or resolved.

Each batch is limited to 500 records and 4 MiB of normalized encoded input, including embedded participants. A source is limited to one million distinct content items and 2 GiB of cumulative accepted encoded input; database/index overhead is additional. Item text, provider envelopes and attachments have separate bounds. Provider adapters must explicitly validate their normalized records and inert locators; registering an adapter does not automatically enable archive ingestion for it.

Internal queries select one provider/account/source and return that source's complete observation, even if another export contains a newer version of the same resource. Search uses literal FTS phrases rather than raw MATCH operators, with author, content-kind and exclusive date filters. Attachment display names are searchable and use the shared privacy detector; attachment contents and remote locators are not fetched. Queries accept at most 4 KiB of text and return at most 200 records per page; resolution is also limited to 200 references.

Pagination preserves exact timestamp ordering and binds its cursor to scope, filters and the completed import generation. A missing or replaced generation rejects old cursors. These are backend interfaces, not a newly enabled archive search screen or importer.

## Source removal and maintenance

Source removal deletes only Retract's imported observations. It never modifies the original export or calls a remote provider. Surviving sources keep their observations and stable resource IDs. Removal retires the source UUID and invalidates its sessions and cursors. The durable outcome contains the removed-item count and `maintenance_pending`; successful logical deletion must remain visible even when later checkpoint/compaction fails.

A future UI must distinguish imported-copy removal from physical maintenance, display pending maintenance honestly, and offer an explicit retry without restoring content or implying remote deletion. Retrying maintenance cannot resurrect a removed source. No destructive archive IPC or maintenance UI is added here.

Identity pruning constructs a memory-only membership set of surviving identities, and `VACUUM` uses memory temporary storage. These operations are **not constant-memory**. Compaction, indexes, WAL growth and recovery staging are additional to the normalized-input quota; low memory/disk failures can leave maintenance pending.

## Verification gates

The focused codec suite creates a real encrypted database, enables FTS5 secure-delete, reopens it, searches and deletes a literal record, and rejects a wrong key. Separate real-file tests cover plaintext preservation, page tampering, missing-codec policy, file-backed temporary-store rejection, disabled extension loading, fixed error codes, and plaintext-canary absence from the database, WAL, and rollback journal.

Linux checks run offline and non-root in the existing Docker build on both arm64 and amd64 with reusable named caches. The required Apple-silicon macOS workflows run the entire synthetic archive suite (including the original codec gate) and injected vault/lease tests before unsigned packaging, check the packaged executable and TDLib runtime dependencies, and reject developer-machine library paths. The benchmark feature is not enabled by ordinary packaging. Linux success is not evidence of macOS linkage: the native test/package job must pass for the final reviewed commit.

The initial dependency gate passed on 2026-09-05 in [Secure build run 33979891484](https://github.com/pryzm-labs/retract/actions/runs/33979891484), at commit `cdcc083e8c604fba58b13ecb85e883276e3ec40a`. Both Linux architectures passed their checks; Apple silicon passed all 12 codec tests and the unsigned app's bundle, signature, architecture, checksum and runtime-dependency validation. This verifies the native dependency foundation, not archive functionality or real Keychain prompt behavior. The final integrated archive implementation must run the gates again.

The repository and recovery-preservation changes were independently reviewed through `6616087`. At that revision, Docker arm64 passed 45 focused archive tests, formatting and strict Clippy. These synthetic tests include rejected WAL/hot-journal byte preservation, supported crash recovery, private staging, partial-copy failure and retained-account validation. Native verification of these later changes remains part of the final integration gate.

Import and query changes were independently reviewed through `a4ca66a`. The full Docker arm64 gate passed 272 application tests plus the frontend/domain suites, formatting, strict Clippy and bundle/public-metadata checks. A separately executed 100,000-item synchronous ingestion fixture demonstrated bounded batch processing; it does not replace the pending full-worker import/query/removal benchmark or final native verification.

## Opt-in synthetic benchmark

Run only in Docker, with no real credentials or Telegram data:

```sh
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=retract_bench_dir=$(mktemp -d) && cargo run --offline --locked --manifest-path src-tauri/Cargo.toml --features archive-bench --example archive_storage_bench -- "$retract_bench_dir"' .
```

The example accepts only an exclusively owned, empty, canonical private disposable directory. Its non-default `archive-bench` feature constructs synthetic adapters/keys internally and uses the real service/repository/query ports. It imports exactly 100,000 items across two 50,000-item sources, verifies complete bounded pagination and 1,000 literal search hits, removes one source, and rechecks all 50,000 survivors plus 500 surviving search hits. It measures the schema's actual FTS, scope/author/kind ordering, identity-membership and reference indexes, along with pruning and memory-backed compaction.

Reported Linux `VmHWM` is the OS's cumulative whole-process peak RSS, not a separate peak for each phase. Exact quiescent phase DB/WAL lengths are reported separately from 10 ms sampled disk high values; sampling can miss short-lived peaks. The measured process includes the worker, runtime, bounded input/page buffers and sampler. This synthetic workload does not establish resource usage for maximum-size records or the million-item/two-GiB source caps.

The pre-rework task-7 Linux arm64 Docker run on 2026-09-06 used the unoptimized dev profile with debug info disabled. It committed 100,000 items / 85,701,212 encoded input bytes in 101.889 s (202 batches), verified all records and 1,000 search hits in 3.032 s, and removed 50,000 items with 50,000 survivors in 23.225 s. Maintenance completed and all survivors plus 500 surviving search hits were rechecked. Worker admission and request-bound logic were subsequently reimplemented from failing tests; these measurements remain historical evidence until the controller runs the final benchmark after review.

| Measurement | Observed bytes |
| --- | ---: |
| Cumulative OS peak RSS after import | 67,047,424 |
| Cumulative OS peak RSS after query | 67,252,224 |
| Cumulative OS peak RSS after removal / final | 227,934,208 |
| Exact DB after import | 314,785,792 |
| Exact DB after removal / shutdown | 149,889,024 |
| Exact WAL at phase boundaries | 0 |
| Sampled DB high | 314,785,792 |
| Sampled total disk high, including rollback journal | 498,715,848 |

This run used the repository's rollback-journal behavior; no WAL growth was observed. The sampled disk values are lower bounds on actual peaks. The increase in process high-water RSS during removal includes both identity pruning and memory-backed `VACUUM`; it does not isolate their individual costs. The benchmark was run once and is not part of standard packaging or recurring gates. Native/manual verification remains separate.
