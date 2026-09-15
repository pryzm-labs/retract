# Discord Archive Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Import a verified current Discord Data Package into Retract's encrypted archive store through a bounded, restart-safe, backend-only pipeline.

**Architecture:** A format-neutral Rust ZIP reader inventories and streams a retained read-only file handle without extraction or network access. A Discord adapter recognizes one evidence-backed JSON profile, normalizes typed records, and feeds versioned batches through narrow additions to the existing encrypted archive service; no production UI or IPC is enabled in this stage.

**Tech Stack:** Rust 2024, exact `zip` 8.6.0 with default features disabled and `deflate-flate2-zlib-rs` only, Serde streaming visitors, SHA-256, Tokio blocking tasks, SQLCipher/FTS5 through existing `rusqlite`, Docker BuildKit checks on Linux ARM64/AMD64, native macOS CI.

**Spec:** `docs/superpowers/specs/2026-09-07-discord-archive-import-design.md`

## Global Constraints

- Support only the single current JSON schema profile proven by an authorized structural specimen; CSV and guessed wrappers fail as unsupported.
- Do not inspect a real export until its owner explicitly supplies its path and authorizes local, content-free structural inspection.
- Never copy a real export into the repository, Docker build context, test fixture, cache, log, diagnostic, or plan.
- The parser has no network, credential, Telegram, UI, Tauri, SQL, or filesystem-extraction dependency.
- No Discord token, login, self-bot, API call, automatic deletion, external URL opening, new production IPC, or user-facing Discord availability.
- Preserve opaque Discord decimal IDs losslessly; never route them through JavaScript numbers.
- Retain the ZIP handle throughout a run, hash exact bytes, stream records, enforce every named ceiling, and never persist an absolute source path.
- Incomplete/failed/cancelled sources remain unsearchable; no invalid message may be silently omitted from a ready import.
- Identical imports deduplicate; different exports remain separate snapshots; missing content never proves remote deletion.
- Parser warning codes are fixed and content-free. Message text, filenames, URLs, IDs, paths, and snippets never enter job logs or diagnostics.
- Existing Telegram behavior, FoundationStore data, job recipes, TDLib, credentials, and frontend remain unchanged.
- Existing archive v1 data migrates through an authenticated encrypted candidate; no plaintext fallback, migration dump, or automatic replacement on failure.
- All project build/test commands run in Docker as non-root with project execution offline. Native macOS validation runs in CI; do not run host Cargo/npm builds.
- Do not prune Docker caches, export build images, perform live Discord/Telegram actions, publish, merge, tag, or release without separate authorization.

---

### Task 1: Format-neutral ZIP reader and private structure probe

**Files:**
- Create: `crates/discord-archive/Cargo.toml`
- Create: `crates/discord-archive/src/lib.rs`
- Create: `crates/discord-archive/src/error.rs`
- Create: `crates/discord-archive/src/limits.rs`
- Create: `crates/discord-archive/src/inventory.rs`
- Create: `crates/discord-archive/src/structure.rs`
- Create: `crates/discord-archive/examples/structure_probe.rs`
- Create: `crates/discord-archive/tests/archive_policy.rs`
- Create: `crates/discord-archive/tests/structure_probe.rs`
- Modify: `Dockerfile`
- Modify: `.github/workflows/secure-build.yml`
- Modify: `scripts/check-provider-boundaries.mjs`
- Modify: `scripts/check-provider-boundaries.node-test.mjs`
- Modify: `THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Produces: `ArchiveInventory::inspect<R: Read + Seek>(reader: R, limits: ArchiveLimits, cancel: &dyn Cancellation) -> Result<ArchiveInventory, ArchiveError>`.
- Produces: `StructureProbe::inspect(&mut ArchiveInventory, selected_entries: &[EntryIndex]) -> Result<StructureReport, ArchiveError>` where the report contains entry paths, JSON key names, container shapes and scalar types/counts only.
- Produces: `Cancellation::is_cancelled(&self) -> bool`; callers can inject an atomic flag without Tokio or Tauri dependencies.
- The structure report type deliberately has no field capable of carrying JSON string/number values or raw byte snippets. Entry paths are shape templates: decimal/mixed identifier segments become typed redaction tokens, and only a closed set of generic container/file tokens is retained.

- [ ] **Step 1: Pin the dependency and its review record**

Add this exact dependency and no defaults:

```toml
[package]
name = "discord-archive"
version = "0.1.0"
edition = "2024"
license = "MIT"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.11"
thiserror = "2"
zip = { version = "=8.6.0", default-features = false, features = ["deflate-flate2-zlib-rs"] }
```

Document the resolved lockfile checksums, MIT license, enabled features, absence of encryption/exotic codecs, supported stored/deflate methods, `ZipArchive::new` central-directory behavior, and reviewed local-header/integrity limitations in `THIRD_PARTY_NOTICES.md` and the spec's evidence section. Add the manifest to Docker's dependency fetch and parser test/fmt/Clippy commands, and to the native macOS job.

- [ ] **Step 2: Write RED archive-policy tests**

Generate archives entirely in test memory. Assert fixed safe error variants for: absolute/traversal/backslash/drive/NUL/control names, case-fold/normalization collisions, file/directory conflicts, duplicate paths, encrypted/unsupported compression, symlink/special Unix modes, multi-disk/prefixed archives, truncated directory, local/central mismatch, overlapping/out-of-file ranges, CRC failure, contradictory ZIP64, too many entries, oversized directory/path/components, per-entry/total sizes, zero-compressed nonempty content, excessive ratio, cancellation, and checked-arithmetic overflow.

```rust
#[test]
fn traversal_is_rejected_before_payload_read() {
    let bytes = fixture::zip_with_path("messages/../../secret.json", br#"{}"#);
    let error = ArchiveInventory::inspect(Cursor::new(bytes), tiny_limits(), &NeverCancel)
        .unwrap_err();
    assert_eq!(error, ArchiveError::UnsafeEntryName);
}
```

- [ ] **Step 3: Run the RED tests in Docker**

Run:

```bash
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path crates/discord-archive/Cargo.toml --test archive_policy' .
```

Expected: compile failure because `ArchiveInventory`, limits and error types do not exist.

- [ ] **Step 4: Implement the bounded inventory**

Implement the named design ceilings in `limits.rs`. Preflight EOCD/ZIP64/directory spans with checked integer arithmetic before constructing the library archive. Validate every entry name/metadata and declared aggregate even when its payload is irrelevant. Wrap consumed entries in an observed-byte/ratio/cancellation reader and require clean EOF/CRC before marking an entry valid. Permit only stored and deflated methods.

```rust
pub trait Cancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

pub struct ArchiveInventory<R> {
    reader: R,
    entries: Vec<ValidatedEntry>,
    limits: ArchiveLimits,
}
```

Do not expose the underlying `zip::ZipArchive`, unchecked offsets, or extraction methods.

- [ ] **Step 5: Write RED structure-probe tests**

Assert the probe returns JSON field names, root/container types, scalar type sets, null/optional occurrence, array item shapes and maximum observed structural depth. Serialize the report and assert sentinel values from strings, numbers and URLs are absent. Assert duplicate JSON keys, invalid UTF-8, scalar/record ceilings, deep ignored fields and cancellation fail safely.

- [ ] **Step 6: Implement the value-free streaming structure probe**

Use custom Serde visitors that retain map keys but replace every value with this closed vocabulary:

```rust
pub enum JsonShape {
    Null,
    Boolean,
    Number { integer: bool, signed: bool },
    String,
    Array(Box<JsonShape>),
    Object(BTreeMap<String, JsonShape>),
    Mixed,
}
```

Reject duplicate keys. Never implement `Serialize` for a type holding raw values; `StructureReport` serializes only entry paths, shapes and counts. Apply byte/token/depth ceilings to selected entries and ignored values.

Add a private-use example executable that accepts exactly one input file and writes the JSON report to stdout. It refuses stdin, directories, symlinks and output paths. Its report templates every non-allowlisted path segment so account names and native IDs cannot appear; tests scan serialized output for sentinels from path names, keys' values, text, URLs, IDs and timestamps.

- [ ] **Step 7: Enforce parser isolation**

Extend `check-provider-boundaries.mjs` so production files in `crates/discord-archive/src` fail if they reference network crates/APIs, Tauri, Keychain/credential modules, Telegram, SQL/rusqlite, browser launching, or filesystem write/create/remove calls. Add controlled-input tests proving each forbidden dependency is detected and ordinary `Read + Seek` code remains allowed.

- [ ] **Step 8: Run Task 1 verification and commit**

Run both parser tests plus parser fmt/strict Clippy in ARM64 Docker, then the full ARM64 `checks` target. Verify `git diff --check` and commit:

```bash
git add crates/discord-archive Dockerfile .github/workflows/secure-build.yml scripts/check-provider-boundaries.mjs scripts/check-provider-boundaries.node-test.mjs THIRD_PARTY_NOTICES.md
git commit -m "feat: add bounded Discord archive reader"
```

### Task 2: Establish and freeze the supported Discord profile

**Files:**
- Create: `src-tauri/test-fixtures/discord-import/README.md`
- Create: `src-tauri/test-fixtures/discord-import/manifest.json`
- Create: `src-tauri/test-fixtures/discord-import/current-json.zip`
- Create: `src-tauri/test-fixtures/discord-import/expected-records.json`
- Create: `crates/discord-archive/src/profile.rs`
- Create: `crates/discord-archive/tests/profile.rs`
- Modify: `docs/superpowers/specs/2026-09-07-discord-archive-import-design.md`

**Interfaces:**
- Produces: `DiscordProfile::detect(inventory: &mut ArchiveInventory<_>) -> Result<ProfileInspection, ArchiveError>`.
- Produces: schema key `discord.data_package.messages_json`, version `1`, and parser-policy key `discord.import_policy.v1`.
- Consumes only a structure report explicitly authorized by the export owner; no private content enters the repository.

- [ ] **Step 1: Obtain explicit structural-inspection authorization**

Ask for one absolute path to a current Discord Data Package ZIP and permission to run the Task 1 value-free probe locally. Do not discover files elsewhere. If permission/path is not supplied, record Task 2 as blocked and stop format-specific implementation without changing the decoder.

- [ ] **Step 2: Probe without retaining the archive**

Run the probe from a temporary Docker container with `--network none`, a read-only bind of that single file, a read-only root filesystem, dropped capabilities, `no-new-privileges`, bounded memory/PIDs and a tmpfs output directory. Copy only the value-free structure report to a private temporary path for review; do not copy the ZIP into the repository or Docker build context. Inspect the report for entry layout, exact field names/types, wrapper variants, timestamps, IDs and attachment representation. Delete the temporary report after synthetic fixtures and provenance hashes are verified.

- [ ] **Step 3: Hand-author synthetic profile fixtures**

Create a deterministic ZIP containing synthetic account identity and verified examples of every observed message context: DM, group DM, guild channel, attachment-only message and empty message collection. Use IDs above `9_007_199_254_740_991`, fixed UTC timestamps, invented text and reserved `example.invalid` URLs. The fixture manifest records only the structural specimen's observation date, Discord documentation URL, profile fields/types, generator command, synthetic file hashes and the explicit statement that no source values were copied.

- [ ] **Step 4: Write RED profile detection tests**

Test the exact accepted roots/required files/JSON shapes. Assert missing/duplicate/contradictory account identity, changed required fields, CSV transcript, mixed wrappers and ambiguous roots return `UnsupportedProfile` or `InvalidProfile`. Assert unknown fields are bounded and ignored.

- [ ] **Step 5: Implement exact profile detection**

Parse only the frozen profile. Return exact entry indexes and typed header information; do not parse message content yet. Use lossless decimal ID token validation and exact timestamp grammar derived from evidence.

- [ ] **Step 6: Verify fixture privacy and commit**

Add an automated fixture audit rejecting email/phone/IP patterns, non-`example.invalid` URLs, unexpected usernames and values absent from the approved synthetic manifest. Run parser profile tests, `npm run check:public-repo`, `git diff --check`, then commit:

```bash
git add crates/discord-archive/src/profile.rs crates/discord-archive/tests/profile.rs src-tauri/test-fixtures/discord-import docs/superpowers/specs/2026-09-07-discord-archive-import-design.md scripts
git commit -m "test: freeze Discord archive profile"
```

### Task 3: Stream typed Discord records

**Files:**
- Create: `crates/discord-archive/src/record.rs`
- Create: `crates/discord-archive/src/reader.rs`
- Create: `crates/discord-archive/tests/reader.rs`
- Modify: `crates/discord-archive/src/lib.rs`

**Interfaces:**
- Produces: `DiscordArchiveReader::open(ArchiveInventory<_>, ProfileInspection, &dyn Cancellation) -> Result<Self, ArchiveError>`.
- Produces: `read_account() -> Result<ExportAccount, ArchiveError>` and `visit_channels(&mut self, sink: &mut dyn RecordSink) -> Result<ReadSummary, ArchiveError>`.
- Produces: `RecordSink::begin_channel(ChannelContext)`, `message(SentMessage)`, and `end_channel(EntryIntegrity)` with backpressure through synchronous return.
- IDs are `DiscordId(String)` validated as canonical positive decimal strings.

- [ ] **Step 1: Write RED record-streaming tests**

From the frozen ZIP assert exact account, channel and message records; deterministic entry/message ordering; multiline Unicode preservation; attachment-only records; large IDs; UTC timestamp normalization; and end-of-entry integrity. Add failures for exponent/float/negative/zero/noncanonical IDs, invalid dates, contradictory author/account, duplicate required keys, oversized raw/decoded records, deep ignored fields, bad CRC after emitted records and cancellation between reads.

- [ ] **Step 2: Run RED tests in Docker**

Run the focused `reader` test; expect compile failure for absent reader/record types.

- [ ] **Step 3: Implement typed streaming visitors**

Deserialize one record at a time into bounded typed fields. Do not create a whole transcript `Vec` or generic raw-record `Value`. Emit channel context before messages and require `end_channel` only after decompressor EOF/CRC succeeds. Return a failed result after a late CRC error even if the sink accepted earlier records; downstream readiness remains gated.

- [ ] **Step 4: Verify bounded behavior and commit**

Run all parser crate tests, strict Clippy/fmt and a heap-sensitive generated 100,000-record reader test that asserts the sink's maximum in-flight record count stays bounded. Commit:

```bash
git add crates/discord-archive
git commit -m "feat: stream typed Discord archive records"
```

### Task 4: Normalize and validate Discord records

**Files:**
- Create: `src-tauri/src/providers/discord/mod.rs`
- Create: `src-tauri/src/providers/discord/locators.rs`
- Create: `src-tauri/src/providers/discord/model.rs`
- Create: `src-tauri/src/providers/discord/normalize.rs`
- Create: `src-tauri/src/providers/discord/tests.rs`
- Modify: `src-tauri/src/providers/mod.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/persistence/archive/lifecycle.rs`

**Interfaces:**
- Produces: `discord_provider_key() -> ProviderKey` with key `discord`.
- Produces: `DiscordPayloadValidator: ProviderPayloadValidator` with policy key `discord.archive_payload.v1`.
- Produces: `DiscordNormalizer::new(scope: Scope, observed_at: DateTime<Utc>)` and typed `account`, `conversation`, `actor`, `content` transformations.
- Produces only `EvidenceState::Archive`, `ConnectionState::Disconnected`, and `ExternalLocationAvailability::Unsupported`.

- [ ] **Step 1: Write RED locator/normalization tests**

Assert exact versioned account/actor/conversation/content/attachment envelopes and canonical keys, stable IDs across two sources, source-scoped observation differences, DM/group/channel mappings, attachment handling, unknown media, fixed observation time, and privacy-detector inputs. Assert display names, URLs, timestamps and guild enrichment do not affect resource IDs.

- [ ] **Step 2: Write RED payload-validator tests**

Reject unknown schema/version, noncanonical IDs, locator/key disagreement, wrong provider/account/kind, active/local URL schemes, oversized metadata, mutable fields in locators, executable recipes, and malformed nested participant/attachment envelopes.

- [ ] **Step 3: Implement minimal Discord codecs**

Use locator schemas `discord.user`, `discord.channel`, `discord.message`, and `discord.attachment`, each at version 1. Canonical message key is the length-delimited tuple `(channel_id, message_id)` encoded by one tested helper—not string concatenation with an ambiguous delimiter. Keep guild/title/URL/timestamps in observation metadata.

- [ ] **Step 4: Register validator lazily**

Change `ArchiveOwner::application` to construct its validator map with `DiscordPayloadValidator`. Assert application construction does not open the archive database or access Keychain. Do not register a Discord `ProviderRegistration` or expose UI capability yet.

- [ ] **Step 5: Verify and commit**

Run focused Discord tests and archive reopen tests in ARM64 Docker; commit:

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/providers/discord src-tauri/src/providers/mod.rs src-tauri/src/persistence/archive/lifecycle.rs
git commit -m "feat: normalize Discord archive records"
```

### Task 5: Migrate archive storage and add import-v2 semantics

**Files:**
- Modify: `src-tauri/src/persistence/archive/schema.rs`
- Modify: `src-tauri/src/persistence/archive/migration.rs`
- Modify: `src-tauri/src/persistence/archive/model.rs`
- Modify: `src-tauri/src/persistence/archive/ingest.rs`
- Modify: `src-tauri/src/persistence/archive/ingest_state.rs`
- Modify: `src-tauri/src/persistence/archive/store.rs`
- Modify: `src-tauri/src/persistence/archive/worker.rs`
- Modify: `src-tauri/src/persistence/archive/remove.rs`
- Modify: `src-tauri/src/persistence/archive/error.rs`
- Create: `src-tauri/src/persistence/archive/import_v2_tests.rs`
- Create: `src-tauri/test-fixtures/archive-v1/store-v1.b64`
- Create: `src-tauri/test-fixtures/archive-v1/manifest.json`
- Modify: `src-tauri/src/persistence/archive/migration_tests.rs`
- Modify: `src-tauri/src/persistence/archive/mod.rs`

**Interfaces:**
- Produces: `NewArchiveImport`, `ArchiveImportResolution`, `ImportDisposition::{Start, Ready, Busy, RetryRequired}`.
- Produces: `ArchiveService::resolve_or_register_import(NewArchiveImport) -> Result<ArchiveImportResolution, ArchiveError>`.
- Produces: `ImportBatchV2 { records: ImportBatch, warnings: Vec<ImportWarningDelta> }` and `append_batch_v2`.
- Produces: `ArchiveService::fail_import(&Arc<ImportSession>, ImportFailureCode) -> Result<ImportProgress, ArchiveError>`.
- Extends `ImportCheckpoint` with immutable `observed_at` and optional content-free `failure_code`.

- [ ] **Step 1: Freeze an authenticated archive-v1 migration artifact**

Using the pre-change schema code and synthetic injected key, create a v1 database containing two providers/sources, observations, FTS, findings, receipts, one interrupted run and cleanup tombstones. Record decoded SHA-256, exact creating commit, schema hash and logical row expectations. Freeze the base64 artifact before changing schema code.

- [ ] **Step 2: Write RED v1-to-v2 migration tests**

Open the frozen artifact through the production migration path and assert every legacy row/receipt/cursor-relevant generation survives, new indexes/tables exist, legacy zero-warning receipt replay still works, unknown/newer schema and candidate failures preserve original bytes, and migration output remains encrypted.

- [ ] **Step 3: Implement schema v2 via encrypted candidate**

Add indexed account/import identity, immutable run observation time, fixed failure code, versioned v2 receipts and warning deltas. Initialize fresh stores directly at v2. Migrate v1 only by building/validating an encrypted candidate and atomic replacement through existing migration plumbing; never edit v1 in place.

- [ ] **Step 4: Write RED atomic-registration tests**

Cover concurrent identical imports, ready/active/failed/cancelled dispositions, same account + new fingerprint, same fingerprint + different profile/policy, stable account identity, removed-source non-resurrection, scope mismatch, and two accounts with colliding display names.

- [ ] **Step 5: Implement store-owned registration**

Validate the provider account payload before canonical lookup, perform lookup/allocation in one transaction, generate non-nil account/source UUIDs inside the store, and return the complete persisted records. Do not accept UUIDs or canonical identity from UI-shaped input.

- [ ] **Step 6: Write RED strict batch/warning/failure tests**

Assert exact duplicate suppression, conflicting duplicate rejection within/across batches, detector-field exclusion from provider comparison, idempotent warning replay, 32-code bound, fixed-code validation, failed-source hiding, preserved counters, stale-session rejection, late cancellation after ready, and legacy append behavior unchanged.

- [ ] **Step 7: Implement opt-in v2 batch and failure transitions**

Keep `append_batch` semantics for legacy/internal tests. `append_batch_v2` computes a versioned receipt digest over normalized provider records plus warning deltas, compares existing source observations before update, and derives privacy fields only after provider conflict comparison. `fail_import` requires the live session and persists `Failed` plus a closed `ImportFailureCode` without content.

- [ ] **Step 8: Verify migration/store and commit**

Run archive migration, ingest, worker, query, remove and Discord tests; full strict Clippy/fmt; verify frozen artifact hashes and `git diff --check`. Commit:

```bash
git add src-tauri/src/persistence/archive src-tauri/test-fixtures/archive-v1
git commit -m "feat: add restart-safe archive import identity"
```

### Task 6: Coordinate Discord import, replay and shutdown

**Files:**
- Create: `src-tauri/src/providers/discord/import.rs`
- Create: `src-tauri/src/providers/discord/progress.rs`
- Create: `src-tauri/src/providers/discord/import_tests.rs`
- Modify: `src-tauri/src/providers/discord/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/persistence/archive/lifecycle.rs`

**Interfaces:**
- Produces: backend-only `DiscordImportOwner::start(file: File) -> Result<DiscordImportHandle, DiscordImportError>`.
- Produces: `DiscordImportHandle::{latest_progress, cancel, wait}`; dropping the handle does not detach the owned task.
- Produces: phases `Inspecting`, `Hashing`, `Registering`, `Importing`, `Verifying`, `Ready`, `Cancelled`, `Failed` with optional totals and committed counters.
- Consumes Task 3 reader, Task 4 normalizer and Task 5 import-v2 service; does not implement public `ImportInspector` or Tauri commands.

- [ ] **Step 1: Write RED happy-path integration test**

Import the frozen synthetic ZIP through the real coordinator and encrypted store. Assert deterministic batches, ready source, complete paged query, expected privacy findings, restart/reopen equivalence, identical-import `Ready`, second-fingerprint snapshot with stable IDs, source-specific observations, removal isolation, and unchanged synthetic Telegram foundation state.

- [ ] **Step 2: Write RED lifetime/failure tests**

Cover cancellation in inventory/hash/JSON/queue/commit, dropped waiters, full queue, parser failure after acknowledged batches, finalization/disk failure, input replacement/change, exact replay, changed policy/profile/file refusal, active identical race, shutdown during every phase, and sticky archive-key/open failure. Assert no orphan worker, late write, repeated Keychain load or ready incomplete source.

- [ ] **Step 3: Implement owned coordinator state**

Store every join handle in `DiscordImportOwner` before returning. Use a latest-value watch channel for progress. Inspect/hash before opening `ArchiveOwner`; after registration create/retry the backend session, replay deterministic batches from zero, verify the retained file metadata and second full hash, then call `finish_import`. Map parser/storage failures to fixed codes and call `fail_import` when a live session exists.

- [ ] **Step 4: Wire shutdown ownership**

Add the owner to `RuntimeState`. Shutdown rejects new starts, signals all imports, drains their parser/coordinator handles, then shuts down `ArchiveOwner`, then clears cached secrets. Add tests for this exact order. Application construction remains side-effect free and no production command can start an import yet.

- [ ] **Step 5: Verify and commit**

Run focused import/lifecycle tests, then full ARM64 Docker checks. Commit:

```bash
git add src-tauri/src/providers/discord src-tauri/src/lib.rs src-tauri/src/persistence/archive/lifecycle.rs
git commit -m "feat: coordinate encrypted Discord imports"
```

### Task 7: Security, resource and release acceptance

**Files:**
- Create: `src-tauri/examples/discord_import_bench.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `Dockerfile`
- Modify: `.github/workflows/secure-build.yml`
- Modify: `scripts/check-provider-boundaries.mjs`
- Modify: `scripts/check-provider-boundaries.node-test.mjs`
- Modify: `docs/ARCHIVE_STORAGE.md`
- Modify: `docs/TEST_PLAN.md`
- Modify: `PLAN.md`
- Modify: `README.md` only to state that Discord remains unavailable until UI work lands; do not list it as supported.

**Interfaces:**
- Produces: opt-in `discord-import-bench` feature and `discord_import_bench` example using generated synthetic data only.
- Produces: CI coverage for parser crate tests/fmt/Clippy on Linux ARM64/AMD64 and native macOS.

- [ ] **Step 1: Add the generated resource benchmark**

Generate deterministic ZIPs of 10,000 and 100,000 messages without checking large fixtures into Git. Run the actual reader/coordinator/encrypted store. Report raw ZIP bytes, decoded bytes, accepted normalized bytes, DB/sidecar/temp disk high-water marks, elapsed phases and whole-process peak RSS. Verify full pagination/findings/restart/removal. The benchmark accepts only a canonical, empty, private temporary directory and no real file path.

- [ ] **Step 2: Run resource and ceiling tests**

Run the ARM64 benchmark in Docker with named cache mounts and `--network=none`. Compare 10k vs 100k to detect whole-transcript memory materialization. Run small injected-limit tests for every production ceiling. Record observed numbers and limitations in `docs/ARCHIVE_STORAGE.md`; do not extrapolate to a million records.

- [ ] **Step 3: Complete repository security checks**

Run fixture privacy audit, dependency feature/license audit, architecture checker including controlled violations, production-bundle exclusion, log-diagnostic sentinel tests, source-tree secret scan, migration artifact hash validation and `git diff --check`. Verify no new command/IPC, network crate, URL fetch/open, credential access from parser, raw archive fixture, absolute path, private content or Discord availability claim.

- [ ] **Step 4: Run full Docker gates**

Run exact `checks` builds with cache-only output for Linux ARM64 and AMD64. Read full summaries: frontend, release, public-repo, production bundle, architecture, all Rust tests, fmt, strict Clippy and doc tests must have zero failures. One existing opt-in 100,000-item archive resource benchmark may remain ignored in normal tests.

- [ ] **Step 5: Obtain independent review and fix findings**

Review each task against the spec and whole-branch diff. Fix Critical/Important findings test-first and repeat affected/full gates. Record any deferred minor with exact scope and why it does not weaken import correctness, privacy, or recovery.

- [ ] **Step 6: Update status and commit acceptance**

Mark only backend Discord import complete. Keep Discord UI/remediation, real private-export validation, publication, merge and release pending. Commit:

```bash
git add Dockerfile .github/workflows/secure-build.yml scripts src-tauri/Cargo.toml src-tauri/examples docs PLAN.md README.md
git commit -m "docs: record Discord importer acceptance"
```

- [ ] **Step 7: Hand off without publishing**

Report branch, commits, exact Docker results, benchmark measurements, structural evidence provenance, known limits and pending native/manual gates. Present the normal finish options. Do not push, merge, tag or release without the user's explicit choice.
