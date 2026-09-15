# Archive storage

Retract contains an internal encrypted archive repository, a lazy bounded backend service, and a backend-only Discord importer. Discord remains unavailable to users: no production import command, source-switching UI, remediation UI or background import registration is enabled. Telegram continues to use its existing live-provider and foundation-store paths; live Telegram message bodies and existing jobs are not copied into this database.

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

The integrated synthetic suites cover rejected WAL/hot-journal byte preservation, supported crash recovery, private staging, partial-copy failure, retained-account validation, bounded imports and queries, source removal, process contention, cancellation and terminal shutdown. The same-worker resource benchmark below is separate from the ordinary suite. See [the test plan](TEST_PLAN.md) for final automated and manual acceptance gates.

## Opt-in synthetic benchmark

Run only in Docker, with no real credentials or Telegram data:

```sh
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=retract_bench_dir=$(mktemp -d) && cargo run --offline --locked --manifest-path src-tauri/Cargo.toml --features archive-bench --example archive_storage_bench -- "$retract_bench_dir"' .
```

The example accepts only an exclusively owned, empty, canonical private disposable directory. Its non-default `archive-bench` feature constructs synthetic adapters/keys internally and uses the real service/repository/query ports. It imports exactly 100,000 items across two 50,000-item sources, verifies complete bounded pagination and 1,000 literal search hits, removes one source, and rechecks all 50,000 survivors plus 500 surviving search hits. It measures the schema's actual FTS, scope/author/kind ordering, identity-membership and reference indexes, along with pruning and memory-backed compaction.

Reported Linux `VmHWM` is the OS's cumulative whole-process peak RSS, not a separate peak for each phase. Exact quiescent phase DB/WAL lengths are reported separately from 10 ms sampled disk high values; sampling can miss short-lived peaks. The measured process includes the worker, runtime, bounded input/page buffers and sampler. This synthetic workload does not establish resource usage for maximum-size records or the million-item/two-GiB source caps.

The Linux arm64 Docker run at reviewed code commit `08a67cc3deb595cd6d4f27664467e550eaf636d1` used the unoptimized dev profile with debug info disabled. It committed 100,000 items / 85,701,212 encoded input bytes in 113.329 s (202 batches), verified all records and 1,000 search hits in 3.043 s, and removed 50,000 items with 50,000 survivors in 22.799 s. Maintenance completed and all survivors plus 500 surviving search hits were rechecked. This run includes the independently reviewed shutdown, reverse-reference and pre-admission fixes, including the conversation-parent reference index; it replaces the earlier development measurements.

| Measurement | Observed bytes |
| --- | ---: |
| Cumulative OS peak RSS after import | 67,018,752 |
| Cumulative OS peak RSS after query | 67,215,360 |
| Cumulative OS peak RSS after removal / final | 227,885,056 |
| Exact DB after import | 314,793,984 |
| Exact DB after removal / shutdown | 149,893,120 |
| Exact WAL at phase boundaries | 0 |
| Sampled DB high | 314,793,984 |
| Sampled total disk high, including rollback journal | 498,728,136 |

This run used the repository's rollback-journal behavior; no WAL growth was observed. The sampled disk values are lower bounds on actual peaks. The increase in process high-water RSS during removal includes both identity pruning and memory-backed `VACUUM`; it does not isolate their individual costs. The benchmark was refreshed once after the final fix review because both admission and schema changed; it is not part of standard packaging or recurring gates. Native/manual verification remains separate.

## Discord backend acceptance

The importer accepts the single frozen `discord.data_package.messages_json` v1 profile described in the [design](superpowers/specs/2026-09-07-discord-archive-import-design.md) and [synthetic provenance manifest](../src-tauri/test-fixtures/discord-import/manifest.json). Its interpretation is deliberately narrow: account-owned sent messages, canonical `u64` IDs, unzoned calendar text checked against the message Snowflake, opaque channel types represented as `Other`, and empty or one inert HTTPS attachment reference. It does not fetch/open links, authenticate an account, expose remediation, infer a complete conversation, or certify unrelated ZIP payload CRCs.

Schema v2 preserves the frozen encrypted v1 artifact, its DDL hash, legacy receipts and existing observations. Import identity resolution, warning receipts, fixed failures, hidden incomplete sources and explicit deterministic retries use the same store and worker as indexed queries/removal. Input identity is checked using the retained regular-file descriptor and two complete hashes. This detects ordinary changes; it is not a guarantee against a malicious same-user race. Cancellation/shutdown retain and drain the owned blocking task before archive/credential shutdown.

The generated benchmark runs exactly 10,000 and 100,000 records through the actual reader, normalizer, import owner, encrypted archive owner, paged query and source-removal service:

```sh
docker buildx build --platform linux/arm64 --target discord-import-benchmark --output type=cacheonly --progress plain .
```

The non-default `discord-import-bench` example accepts one empty canonical private temporary directory, validates ownership/mode/symlinks, and accepts no archive path, credentials or corpus-size override. It generates four deterministic stored ZIP entries with one record buffered at a time. It verifies every message and stable pagination ordering, email findings and indexed hits, closes/reopens the encrypted store, confirms identical-import reuse without reparsing, removes the source, and verifies the source is gone. Generated inputs/stores are removed and the Docker target verifies that its temporary directory is empty before removing it. Named Cargo caches hold compilation output only; the target exports no runnable image.

Raw ZIP bytes, decoded entry bytes (the sum of uncompressed generated entry payloads), accepted normalized bytes and disk lengths are separate measurements. Physical parser read bytes include repeated inspection/typed reads; hashing bytes include two passes. Disk high-water values are sampled every 10 ms, count DB, sidecars, nested temporary files and the generated ZIP, and may miss transient peaks. RSS is Linux `VmHWM`, a cumulative whole-process high water including generation, runtime, workers, sampler, query, reopen and removal. The 100k result shares a process with the earlier 10k run; its RSS is not an independent per-corpus peak. The benchmark uses stored ZIPs and one small-record channel, not a compression throughput or worst-case-record workload. Neither this comparison nor the source quota establishes million-record performance. Classic ZIP/selection/token/retained budgets may bind before the higher storage quotas.

### Ceiling coverage

All production ceilings remain unchanged. `ArchiveLimits::default`, archive model constants and `ImportLimits::default` are authoritative. Parser options reject zero or increases; private `AdmissionLimits` draws from existing storage constants and permits only smaller positive limits for the same production admission path. Existing exact-boundary tests remain alongside the small-limit checks.

| Resource | Production ceiling | Evidence (synthetic tests) |
| --- | --- | --- |
| ZIP bytes; directory bytes/count | 4 GiB; 32 MiB / 100,000 | `every_declared_limit_is_enforced_before_consumption` |
| Entry path bytes/components | 1,024 / 16 | Same injected preflight test; unsafe/collision/range tests |
| Entry/all-declared/pass-expanded bytes | 2 GiB / 16 GiB / 4 GiB | Declared-limit tests; repeated-consumption observed-byte and lying-size tests |
| Expansion ratio; nonempty zero-compressed | 1,000:1; reject | `zero_compressed_nonempty_and_excessive_ratio_are_rejected` |
| JSON depth/scalar/token | 64 / 1 MiB / 1,000,000 | Structure, profile and typed-reader injected ignored-field tests |
| Raw/decoded record | 8 MiB / 2 MiB | Structure/reader raw/decoded tests with positive reset controls |
| Display/selected contexts | 4 KiB / 40,000 selected entries | Exact UTF-8 display tests; selection/profile/forged-inspection tests |
| Retained structure/headers | 32 MiB accounted | Cross-record unique keys, incremental grammar members, pre-allocation/growth and retained-budget handoff tests |
| Batch count/encoded bytes | 500 / 4 MiB | `injected_admission_limits_cover_every_batch_item_and_warning_ceiling`; exact count/byte boundary tests |
| Text/envelope/attachments | 1 MiB / 64 KiB / 100 | Same small admission test; nested item and synchronous pre-queue rejection tests |
| Queued batches | 2 | Real worker tests hold both slots and cancel/reject before allocation |
| Query text/page/resolve | 4 KiB / 200 / 200 | Query boundary/cursor/resolve tests and full-queue admission tests |
| Source distinct items/accepted input | 1,000,000 / 2 GiB | Injected item/byte quotas across updates, cancellation, replay and retry |
| Warning entries/counts/failure codes | 32 / signed SQLite range / closed enums | Small warning admission; zero/overflow/replay/unknown-code and fixed-failure tests |
| File identity/size/content | Retained regular file; exact before/after identity/hash | Symlink/special/change/replacement/hard-link and coordinator cancellation tests |

The named 40,000 context ceiling currently bounds all selected evidence entries (`2 + 2 * channels`), so it conservatively admits at most 19,999 paired channels. All ZIP64 forms remain rejected; classic ZIP's representable count can bind before the 100,000 entry ceiling. Retained structure is conservative allocation accounting, not measured RSS.

### Observed Discord benchmark

The ARM64 Docker run on 2026-09-10, build `aaeycmix7jeyhaq956hjcp6ho`, used the unoptimized dev profile with debug information disabled, UID 10001 and no network after locked dependency fetch. Both complete corpora passed all pagination, findings/search, reopen/reuse and removal assertions. Neither corpus reported a sampling error; temporary-file high water was zero and the generated directory cleanup succeeded.

| Measurement | 10,000 records | 100,000 records |
| --- | ---: | ---: |
| Raw ZIP bytes | 1,343,670 | 13,430,670 |
| Decoded entry bytes | 1,343,114 | 13,430,114 |
| Accepted normalized bytes | 10,526,027 | 105,246,707 |
| Committed batches | 21 | 201 |
| Whole-process RSS after import, bytes | 69,029,888 | 70,479,872 |
| Whole-process final RSS, bytes | 70,479,872 | 73,781,248 |
| Exact DB after import / sampled DB high, bytes | 39,567,360 | 394,801,152 |
| Sampled sidecar high, bytes | 39,624,840 | 395,722,896 |
| Sampled total disk high, bytes (includes ZIP) | 80,535,870 | 803,954,718 |
| Exact DB after removal/shutdown, bytes | 258,048 | 270,336 |
| Generation, seconds | 0.045 | 0.451 |
| Inspection/hash/import/final verification, seconds | 11.309 | 141.089 |
| Full pagination/findings/indexed query, seconds | 0.340 | 3.073 |
| Reopen/repeat/full pagination, seconds | 0.909 | 9.152 |
| Source removal/shutdown, seconds | 2.159 | 34.958 |

The tenfold input increase added about 1.45 MB to cumulative RSS at the import boundary, rather than tracking the added transcript bytes. Together with the typed reader's bounded-allocation test and one-record generator, this is evidence against whole-transcript materialization for this corpus; it is not a universal memory bound. Sidecar peaks include rollback journals and compound transaction/removal work, with zero sidecars at the reported quiescent boundaries. The benchmark removes the only source in each store; the earlier archive-storage benchmark separately measures removal with surviving sources.
