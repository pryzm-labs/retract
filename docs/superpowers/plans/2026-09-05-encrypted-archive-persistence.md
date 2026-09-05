# Encrypted Archive Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide a lazily opened, independently keyed archive store with bounded ingestion, indexed queries, and truthful local-source removal, verified using synthetic records.

**Architecture:** SQLCipher owns archive identity/provenance/content; `FoundationStore` continues owning Telegram identities and jobs. A synchronous, locked repository runs behind a bounded worker interface, with no new destructive IPC or real importer registered. Provider payload validation stays adapter-owned.

**Tech Stack:** Existing Rust/Tauri, `retract-domain`, exact `rusqlite = 0.40.2` with default features disabled and `bundled-sqlcipher-vendored-openssl`, SQLite FTS5, consolidated macOS Keychain vault, and existing Docker/GitHub native build gates.

**Spec:** `docs/superpowers/specs/2026-09-05-encrypted-archive-persistence-design.md`.

## Implementation checkpoint — 2026-09-05

Tasks 1 and 2 have locally committed implementations and independent code reviews: SQLCipher dependency/runtime gate (`aeaceda`) and lazy verified vault key (`18b8dad`). The full Linux arm64 and amd64 Docker gates pass 406 tests at the latter code revision, with formatting, Clippy, production-bundle and repository checks passing.

The stage is **not complete**. Feature-branch push and Secure build dispatch are authorized. After a reviewed test-only temporary-root normalization fix, [Secure build run 33979891484](https://github.com/pryzm-labs/retract/actions/runs/33979891484) passed at `cdcc083e8c604fba58b13ecb85e883276e3ec40a`: Linux arm64/amd64 checks, all 12 native macOS codec tests, and unsigned Apple-silicon bundle/link validation. The dependency gate is now clear for Task 3. Tasks 3–7 remain unimplemented at this checkpoint. Task 7's app-wide credential lease is mandatory before production archive activation. Native Keychain prompt/permission testing is a separate manual gate; no real credentials or Telegram data were used in these tests, and no production archive caller or importer is enabled.

## Global Constraints

- Archive-only SQLCipher storage; never copy live Telegram bodies into the archive database or migrate existing jobs/session files.
- SQLCipher failure on a required target stops schema work; no plaintext SQLite/FTS fallback.
- Required native gate: Linux arm64/amd64 Docker execution and Apple-silicon macOS runner execution/link/package verification. No Windows release claim.
- Source-pin native dependencies, record checksums/crypto backend/compiler flags/licenses, and reuse named Docker caches. Project execution remains non-root/offline in Linux builds.
- The archive-index key is independent, never an environment variable or IPC field. Production archive storage requires an approved OS credential store; non-macOS tests inject disposable keys.
- Preserve existing vault secret values and cached access. Upgrade the existing consolidated item lazily on first archive use; unknown versions are never overwritten.
- Before enabling the production archive factory, serialize credential ownership across app processes, not just archive database ownership. All Telegram and archive vault consumers share a lazy app-wide credential lease for the cached-vault lifetime. Another process/profile fails closed without prompting or writing the vault; an archive-only database lock cannot protect a Telegram-only writer's stale cache.
- Source observations are separate from account-scoped canonical resource identity. Deleting one source preserves every surviving source's observations and resource IDs.
- All query/import/removal calls bind provider/account/source. Archive evidence never authorizes remote actions.
- Initial enforced limits: 500 records and 4 MiB encoded data per batch; 1 MiB searchable text per content item; 64 KiB per provider metadata envelope; 100 attachments per item; two queued batches; 32 distinct warning codes with aggregate counts; one million content items and 2 GiB cumulative normalized encoded input per source; 4 KiB query text; 200 items per page.
- Limits count committed progress across retry. Database/index overhead is additional to the 2 GiB input cap. Test a 100,000-item synthetic corpus and record memory/disk growth.
- Main DB, FTS, WAL/journal, migrations, temporary stores and logs must not expose plaintext content. Core and FTS secure-delete are separate requirements.
- No raw archive retention, archive path logging, real ZIP parser, network fetching, archive/live identity auto-linking, UI source switcher, or new destructive IPC in this stage.
- Production source removal deletes only Retract's imported copy, not the export or remote data. Interrupted compaction stays pending and cannot restore logical results.
- Existing Telegram tests, v2 lifecycle, bundle checks, release metadata, formatting and Clippy remain green. Do not access real Telegram/Keychain/data for tests.

## Files and interfaces

`src-tauri/src/persistence/archive/{codec,error,model,schema,store,ingest,query,remove,worker}.rs` each owns the indicated responsibility; `mod.rs` exposes only bounded backend interfaces. Keep tests in adjacent `*_tests.rs` modules and synthetic fixture helpers in `test_support.rs`. `secure_store/vault.rs` may hold the extracted vault codec to keep the existing file focused. `docs/ARCHIVE_STORAGE.md` documents on-disk behavior, limitations, provenance and verification.

The dependency gate supplies `ArchiveKey`, `ArchiveError` and the internal keyed-connection constructor. Later storage interfaces use those types, `retract-domain` records and the existing `ProviderPayloadValidator` (structural archive validation never creates an authenticated `ActiveContext`). The importer/session structs and the worker methods below are backend-only; they are not added to Tauri command registration.

## Verification commands

Focused Linux commands use the existing `focused-checks` target, for example:

```sh
docker buildx build --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path src-tauri/Cargo.toml persistence::archive::codec_tests' .
npm run container:check
npm run container:check -- --platform linux/amd64
npm run container:check -- --platform linux/arm64
```

Regenerate a changed Cargo lockfile inside the dependency/toolchain container, never by running a host compiler or uploading application data. If BuildKit has retained a fetch layer but lost its registry mount, repopulate dependencies with `--no-cache-filter dependencies,checks`; do not enable network access for compilation or prune unrelated caches.

---

### Task 1: Establish the pinned SQLCipher native gate

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/src/persistence/mod.rs`, `.github/workflows/secure-build.yml`, `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`, `scripts/package-unsigned-macos.sh`, `scripts/verify-app-contents.sh`, `scripts/release-metadata.node-test.mjs`, `THIRD_PARTY_NOTICES.md`.
- Create: `src-tauri/src/persistence/archive/{mod,error,codec,codec_tests}.rs`, `docs/ARCHIVE_STORAGE.md`, `vendor/sqlcipher/LICENSE.txt`, `vendor/sqlcipher/provenance.json` and corresponding OpenSSL notice if required by the selected vendored source.

**Interfaces:**
- Produces `ArchiveKey(Zeroizing<[u8; 32]>)`, constructed from backend-held bytes and never implementing revealing `Debug`, `Display`, or serialization.
- Produces `ArchiveError` with safe fixed codes: `UnsupportedCodec`, `UnavailableKey`, `InvalidStore`, `UnsupportedSchema`, `StoreInUse`, `ScopeMismatch`, `InvalidRecord`, `LimitExceeded`, `IncompleteSource`, `StaleCursor`, `Cancelled`, `StorageFailure`, `CleanupPending`; no native SQL/error strings or paths in its display.
- Produces internal `open_keyed(path: &Path, key: &ArchiveKey, create: bool) -> Result<rusqlite::Connection, ArchiveError>`; it is not an unrestricted application export.

- [ ] Write tests first against an explicit unsupported constructor stub, then run the focused test and record its expected failure. This establishes the missing backend connection policy, not a compile-error-only RED gate:

```rust
#[test]
fn keyed_connection_reopens_encrypted_fts_and_rejects_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.db");
    let key = ArchiveKey::new([0x42; 32]);
    let db = open_keyed(&path, &key, true).expect("SQLCipher gate must open");
    db.execute_batch("CREATE VIRTUAL TABLE gate_text USING fts5(body);
        INSERT INTO gate_text(gate_text, rank) VALUES('secure-delete', 1);
        INSERT INTO gate_text(body) VALUES('syntheticcanary');").unwrap();
    drop(db);
    assert!(open_keyed(&path, &ArchiveKey::new([0x43; 32]), false).is_err());
    let db = open_keyed(&path, &key, false).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM gate_text WHERE gate_text MATCH 'syntheticcanary'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    db.execute("DELETE FROM gate_text", []).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM gate_text WHERE gate_text MATCH 'syntheticcanary'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}
```

- [ ] Add the pinned binding and generate the lockfile with the existing container. Inspect the actual locked `libsqlite3-sys` SQLCipher amalgamation/build script and vendored OpenSSL source; record their package checksums, upstream revision/source identifiers and build flags in provenance. Do not infer a SQLCipher release from the Rust wrapper version.
- [ ] Implement the constructor: key before schema access, verify cipher version/provider and actual schema read, require in-memory temp stores, foreign keys, full synchronous writes, core secure-delete and supported FTS. Disable extension loading and query logging; use strict open flags and reject existing symlink/nonregular/plaintext/wrong-key paths without replacing them. Detect missing-file `create=false` without creating anything. Key material is wiped on drop and not formatted into diagnostics.
- [ ] Add separate tests for canary-free main/WAL/journal bytes, tampered encrypted pages, plaintext input preservation, missing-codec rejection, unsupported temp-store configuration, and error redaction. Assert bytes of rejected existing files are unchanged.
- [ ] Bundle the required license notices and provenance as explicitly allowed app contents; update real release-validator fixture tests to accept exactly these new files and still reject unrelated files. Add the focused codec tests to the native macOS job before the existing unsigned packaging step in both workflows. Verify the executable links only expected system/runtime dependencies, not a developer-machine OpenSSL path.
- [ ] Run focused RED/GREEN then all existing Linux gates on both architectures. Controller runs the feature branch's read-only secure-build workflow only if push/dispatch authority is granted. Record macOS test and packaging outcome against the exact commit. No release workflow dispatch or tag creation.
- [ ] Commit the dependency gate and its evidence. **Do not proceed to Task 3 or any schema work until the required native gate is green.** Task 2's independent vault codec/tests may proceed after Task 1's local implementation review while runner authority is pending; neither actual user vault migration nor schema creation occurs. A missing runner/authority is an explicitly reported gate, not permission to weaken it.

### Task 2: Add the lazy versioned vault key without prompt regressions

**Files:**
- Modify: `src-tauri/src/secure_store.rs`.
- Create: `src-tauri/src/secure_store/vault.rs`, `src-tauri/src/secure_store/vault_tests.rs` if extracting the codec is needed; otherwise keep small additions next to the existing tests.

**Interfaces:**
- Consumes `ArchiveKey` from Task 1.
- Produces crate-internal `load_archive_index_key() -> Result<ArchiveKey, AppError>` on macOS only, using the existing synchronized vault item/cache. No non-macOS file-key implementation.
- Version 2 vault names: `telegram/legacy-profile/api-hash`, `telegram/legacy-profile/tdlib-database-key`, `retract/job-store-key`, `retract/content-index-key`. These preserve today's single profile-backed fields; do not guess a native account ID during migration.

- [ ] Preserve fixed `RTRCTV1` fixtures and write RED behavior tests using an injected vault I/O boundary that never touches Keychain. Verify old secrets survive adding the new key, failed writes/readback do not publish the candidate, and repeated requests reuse one cached key:

```rust
let original = fixture_v1_vault();
let mut io = MemoryVaultIo::from_bytes(original.clone());
io.fail_next_write();
assert!(archive_key_with_io(&mut io).is_err());
assert_eq!(io.persisted_bytes(), original);
io.allow_writes();
let first = archive_key_with_io(&mut io).unwrap();
let second = archive_key_with_io(&mut io).unwrap();
assert_eq!(first.expose_for_test(), second.expose_for_test());
assert_eq!(io.committed_write_count(), 1);
assert_eq!(io.decoded_telegram_secrets(), literal_legacy_secrets());
```

All named helpers here belong solely in the test module; production code owns a small vault transaction function, not test-only counters or cleanup methods.

- [ ] Implement a strict bounded namespaced codec with magic `RTRCTV2`. Reject duplicate/unknown/malformed entries and versions; retain zeroizing buffers. Continue writing v1 for Telegram-only users until first archive use, and retain v2 once upgraded. Use the same Keychain service/account item; older builds reject the unknown header instead of seeing an absent item and creating a fresh vault.
- [ ] Stage candidate state, persist it, verify readback, then publish it in the synchronized cache. Preserve the previous cached usable secrets on archive write failures. Distinguish uncertain successful-write/readback-failure conservatively: retry reads existing data and never rotates a key that might already own a database.
- [ ] Test denied/unavailable vault, concurrent requests, unknown v2 input preservation, all existing legacy-key migration paths and a late API-hash edit retaining the content key. Run focused vault tests and the full container gate, then commit. Native prompt/permission behavior remains a named manual gate; no real Keychain test is authorized.

### Task 3: Add the encrypted scoped schema and store lifecycle

**Prerequisite:** Task 1's Linux arm64/amd64 and native macOS codec/link/package gate has passed on the committed dependency implementation. No schema implementation while that gate is pending.

**Files:**
- Create: `src-tauri/src/persistence/archive/{model,schema,store,store_tests,test_support}.rs`.
- Modify: `src-tauri/src/persistence/archive/mod.rs`.
- Modify narrowly: `src-tauri/src/persistence/model.rs` for a crate-internal borrowed canonical-string accessor on `VerifiedNativeAccountIdentity`; preserve existing validation and avoid public serialization.

**Interfaces:**
- `ArchiveStore::open(path: PathBuf, key: ArchiveKey, validators: BTreeMap<ProviderKey, Arc<dyn ProviderPayloadValidator>>) -> Result<Self, ArchiveError>`.
- Internal production variant `ArchiveStore::open_with_key_loader(path, validators, load: impl FnOnce() -> Result<ArchiveKey, ArchiveError>) -> Result<Self, ArchiveError>` acquires the process lock before invoking the loader. The explicit-key entry point delegates through the same lock path. This prevents two app processes from creating competing index keys before either owns the store.
- `ArchiveStore::register_source(account: AccountRecord, source: SourceRecord) -> Result<SourceRecord, ArchiveError>`; archive-only validated source, stable persisted identity, duplicate native identity with conflicting UUID rejected.
- Persist the adapter-verified canonical account identity through that internal accessor, never by formatting `Debug` or treating raw provider payload JSON as canonical identity. Cover semantically equal native identities with distinct payload encodings in duplicate-account tests.
- `ArchiveStore::source(scope: &Scope) -> Result<SourceRecord, ArchiveError>` and internal transactional access used only by adjacent modules.
- Test support creates complete literal synthetic provider/account/source/locator records, including native IDs `9007199254740992`, `9007199254740993` and `message:part/0007`.

- [ ] Write RED real-file tests for create/reopen, wrong key/schema and scope rejection, duplicate account identity, two-process locking, and no effect on a neighboring `jobs.enc` sentinel. Reject a live source without creating content tables for it.
- [ ] Implement schema version 1 and application binding inside the authenticated database. Tables: `schema_migrations`, `accounts`, `sources`, `resource_identities`, `conversation_observations`, `actor_observations`, `content_observations`, `attachments`, `privacy_findings`, `import_runs`, `import_warnings`, `cleanup_tasks`, and external-content `content_fts`.
- [ ] Use account-scoped unique canonical identities; observation primary keys include source and resource ID. Foreign keys include provider/account/source ownership. Keep native payloads as validated envelopes. Put FTS rowids behind internal integer observation keys while all application IDs stay UUID strings. Enable external-content FTS maintenance in the same transaction as observation writes.
- [ ] Hold the process lock for the connection lifetime; serialize writes and maintenance. Forbid symlink paths and insecure file permissions. New schema creation is transactional; unsupported/corrupt existing files remain unchanged. Store actor/reply references without requiring out-of-order imported referenced content to exist yet, but never permit a proven foreign-scope reference.
- [ ] Reuse only adapter validation for opaque payload interpretation. Structural archive identities never create live connection state or remediation authority. Validate every provider registration before publication.
- [ ] Run focused store tests and full container gate; commit the schema/lifecycle unit.

### Task 4: Implement bounded transactional import and recovery

**Files:**
- Create: `src-tauri/src/persistence/archive/{ingest,ingest_tests}.rs`.
- Modify: `src-tauri/src/persistence/archive/{model,mod,store}.rs` and test support.
- Modify narrowly: `src-tauri/src/persistence/model.rs` to extend `ProviderPayloadValidator` with adapter-owned normalized conversation, actor and content validation hooks, including nested provider metadata and attachment locator envelopes. Defaults reject archive ingestion for providers that have not implemented the hooks; preserve existing foundation-store validation paths.

**Interfaces:**
- `ImportBatch { conversations: Vec<ConversationRecord>, actors: Vec<ActorRecord>, contents: Vec<ContentRecord> }`.
- `ImportSession { id: Uuid, scope: Scope, fingerprint: String, schema_profile: VersionedPayload }` is backend-issued, private mutation authority, not a frontend-deserializable grant.
- `ImportProgress { phase: ImportPhase, committed_items: u64, committed_bytes: u64 }`; phases `Importing`, `Ready`, `Interrupted`, `Cancelled`, `Failed`.
- `begin_import(&mut self, scope: &Scope) -> Result<ImportSession, ArchiveError>`; `append_batch(&mut self, session: &ImportSession, batch: ImportBatch) -> Result<ImportProgress, ArchiveError>`; `finish_import`, `cancel_import`, and `retry_import` all require the matching scope/session/fingerprint/schema and return durable progress.
- Existing payload-envelope checks establish structure, not provider semantics. Ingestion must call the new typed-record adapter hooks as well as neutral validation, without interpreting opaque provider payloads in SQL code. Test rejection by a validator with no archive hook support and rejection of unknown nested metadata/attachment locator versions; synthetic supported adapters own those interpretations and use a matching validation policy key.

- [ ] Write RED synthetic tests proving incomplete sources are unavailable to normal queries, identical replay does not duplicate records, and a failed second batch preserves first-batch content/counters/checkpoint exactly. Use failpoints at database/I/O boundaries in tests only.
- [ ] Implement shared limits copied from Global Constraints. Count all record types toward batch record size, use bounded encoded-size counting before queue/copy, check text/metadata/attachment limits and checked arithmetic. Source item count counts distinct content observations, while cumulative committed encoded bytes charge each accepted non-replay batch; exact retry replay uses a persisted batch sequence/digest to return the original checkpoint without charging twice.
- [ ] Within one transaction validate the session, upsert validated observations and FTS/findings, and advance counters/sequence. Reject conflicting duplicate records inside one batch. Preserve another source's earlier observation. Findings are derived using the existing Rust detector through a small normalization adapter; record detector version and never trust frontend-supplied findings as authority.
- [ ] Reuse `cleaner_domain::detect_sensitive_data(text, kind)` for content text and inert attachment display names, translating enums outside `retract-domain`. Use a source-derived detector-version fingerprint or an explicitly versioned constant maintained with the detector; do not duplicate detection regexes. Index attachment display names separately from message text; do not index arbitrary provider metadata or fetch attachment locators.
- [ ] Publish Ready only after final relational validation; unresolved optional references are preserved as inert IDs, while missing mandatory actor/conversation observations fail finalization. Cancel stops later batches; reboot marks nonterminal runs interrupted without auto-resume. Explicit retry revalidates exact provenance and durable limits. Bound warnings to aggregate safe codes.
- [ ] Test 500/501 records, 4 MiB boundary, text/metadata/attachment limits, cumulative limits via small injected ceilings, stale sessions, cross-account records, interrupted restart and cancellation. Run focused import tests and the full gate; commit.

### Task 5: Add archive queries through the normalized query port

**Files:**
- Create: `src-tauri/src/persistence/archive/{query,query_tests}.rs`.
- Modify: `src-tauri/src/persistence/archive/{model,mod}.rs`.

**Interfaces:**
- `ArchiveSearch { scope: Scope, text: String, kinds: Vec<ContentKind>, author: Option<ActorId>, before: Option<DateTime<Utc>>, after: Option<DateTime<Utc>>, cursor: Option<String>, limit: u32 }`.
- Internal synchronous `search(&self, request: ArchiveSearch) -> Result<Page<ContentRecord>, ArchiveError>`, `list_conversations(ConversationQuery) -> Result<Page<ConversationRecord>, ArchiveError>`, `resolve(ResolveRequest) -> Result<Vec<ContentRecord>, ArchiveError>`.
- The asynchronous `QuerySource` adapter is installed on Task 7's bounded worker; it maps existing `ContentQuery` to empty optional archive filters without changing Telegram contracts.

- [ ] Write RED tests with independently listed expected IDs for literal keywords, phrases, punctuation/quotes, provider/account/source isolation, hidden incomplete sources, and two equal-timestamp pages. For example search `alpha` in source A must return only A's observation even when source B contains a newer edit of the same resource.
- [ ] Include a file whose inert display name contains a synthetic email and a keyword absent from its body: keyword search must find the content once, the shared detector must flag the email, and source removal must remove both terms from FTS.
- [ ] Implement parameterized SQL. Translate text to a deliberately bounded quoted FTS expression; never interpolate user SQL or expose arbitrary MATCH operators. Empty query lists bounded recent content. Validate date range, page/query limits and source readiness before querying.
- [ ] Cursor encodes a version, full scope, normalized filter digest, source generation, and last timestamp/ID. Decode with strict size/schema validation; mismatched query/scope/generation returns `StaleCursor`. Results reconstruct complete normalized records and preserve archive evidence and provider envelopes.
- [ ] Test update removes old search terms, a forged foreign resolve ref is rejected, stale cursor after source removal fails, hostile input stays inert and no query/log emits text to diagnostics. Run focused query tests and full gate; commit.

### Task 6: Remove exact local sources and recover cleanup/migrations

**Files:**
- Create: `src-tauri/src/persistence/archive/{remove,remove_tests,migration_tests}.rs`.
- Modify: `src-tauri/src/persistence/archive/{model,schema,store,mod}.rs`.

**Interfaces:**
- `RemovalOutcome { removed_items: u64, maintenance_pending: bool }`.
- Internal `remove_source(&mut self, scope: &Scope) -> Result<RemovalOutcome, ArchiveError>` and `retry_cleanup(&mut self, scope: &Scope) -> Result<RemovalOutcome, ArchiveError>`; backend infrastructure only, not a new native command.

- [ ] Write RED tests where two sources share resource identities but have different observations. Remove A, reopen, and verify B's full records and IDs plus a neighboring export-file sentinel are unchanged. Inject failure after logical removal but before vacuum and prove search cannot restore A.
- [ ] Atomically mark removing, invalidate generation, remove scoped observations/FTS/attachments/findings/checkpoints, and retain only shared identities. Persist content-free cleanup work before committing logical removal. Use both core and FTS secure-delete.
- [ ] Close/drain readers under the store worker lock, checkpoint/truncate WAL and vacuum. Clear pending status only after verified maintenance success. Retry is idempotent and never rereads the original archive or reconstructs deleted content. Missing/already-removed scope returns a truthful stable outcome without changing other sources.
- [ ] Test a real encrypted candidate migration using a test-only old schema fixture: authenticate old, build and validate encrypted candidate, close/checkpoint, atomic switch. Failure before switch preserves original; ambiguous/interrupted artifacts fail closed; valid active plus obsolete candidate does not resurrect older content. Do not invent a production schema upgrade with no predecessor—ship reusable guarded migration plumbing and fixture-verified behavior.
- [ ] Test disk-full/permission/commit failures, pending cleanup restart, canary-free artifacts and FTS term erasure through actual SQL inspection. Run focused tests and full gate; commit.

### Task 7: Expose lazy bounded backend service and complete integration gates

**Files:**
- Create: `src-tauri/src/persistence/archive/{worker,worker_tests,lifecycle_tests}.rs`, `src-tauri/src/secure_store/{vault_lock,vault_lock_tests}.rs`, and `src-tauri/examples/archive_storage_bench.rs`.
- Modify: `src-tauri/src/persistence/archive/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/secure_store.rs`, `docs/ARCHIVE_STORAGE.md`, `docs/TEST_PLAN.md`, `docs/THREAT_MODEL.md`, `README.md`, `CHANGELOG.md`.

**Interfaces:**
- `ArchiveService::open(...)` spawns/owns the repository worker off runtime threads and returns `Arc<ArchiveService>`; macOS production factory obtains the application-owned path and Task 2 key lazily, other targets return `UnavailableKey` without creating files.
- The macOS factory uses `<app_local_data>/archives/content.db` and Task 3's loader variant; key retrieval/creation occurs only after the worker holds the process lock. The path comes from the app handle, not IPC or an environment variable.
- Bind the credential lease to the same explicit application-owned root during backend setup; do not guess the root by stripping profile names. Binding alone creates no files and reads no credentials. The existing synchronized vault acquires an exclusive, fail-fast process lease before its first I/O and holds it until cached secrets are cleared. Every Telegram and archive vault consumer uses it; profile locks and archive-store locks are acquired before the vault lease, and the vault never calls back into those stores. Keep the non-macOS production credential behavior unchanged.
- Worker commands carry typed requests and single response channels; two pending batch slots maximum, try-send/backpressure before accepting an additional batch, shutdown closes repository and drops keys/lock. No arbitrary SQL command variant.
- `ArchiveService` exposes import/query/removal backend methods corresponding to Tasks 4–6. `ArchiveQuerySource` implements existing `QuerySource` through this service; no Telegram gateway/native remediation dependency.

- [ ] Write RED behavior tests: construction/normal Telegram bootstrap does not open archive files or request archive secrets; first requested archive open does; a slow batch does not block an independent Tokio timer; a third queued batch is rejected/backpressured without copying its data; shutdown cancels unstarted work and releases the lock.
- [ ] Before wiring the production archive factory, add real temporary-file/process tests for the shared credential lease: two profiles/processes cannot simultaneously own or mutate the consolidated vault, losing acquisition performs no vault I/O, no stale v1 writer can overwrite a newly persisted content key, and shutdown releases ownership only after archive/store work is drained. Validate lock-file type, owner-only permissions and symlink rejection; never log its path. Existing cached reads and archive-error isolation tests must remain green. Tests use injected vault bytes, not Keychain. This is a required safety gate, not optional hardening.
- [ ] Implement the bounded actor/service; preserve typed scope/session authority and map safe errors without SQL/path/text interpolation. Do not hold an async lock across blocking SQLite or credential operations. Source removal invalidates queued stale requests; cancellation observes only committed progress. Keep public production constructors separate from injected disposable-key test/example setup.
- [ ] Cancellation must set a shared per-session atomic cancellation signal before waiting for a command-queue slot. Check it before each queued batch and before commit; an already committed in-flight batch remains committed. Persist the cancelled state on the worker without allowing queued append commands ahead of that persistence to run mutations. Test cancellation while both queue slots are occupied.
- [ ] Exercise full synthetic provider lifecycle through real ports: ingest two scopes, search, restart, explicit retry, source removal, wrong-scope requests. Confirm no additional destructive Tauri handler or real archive provider is registered.
- [ ] Add a documented opt-in 100,000-item synthetic benchmark using the same service path; report peak process RSS, DB/WAL size, elapsed import/query/removal and committed progress. Enforce bounded pages/queues and assert total records/search truth. Do not run the corpus repeatedly during small fix iterations.
- [ ] Update user documentation honestly: archive backend foundation only; no Discord/X support claim, no forensic-erasure promise, no automatic Telegram indexing. Record source dependency provenance, key/vault downgrade implications, pending maintenance UX contract, and manual native Keychain/privacy gates. Require quitting all older Retract copies before first archive use: an already-running older binary cannot be made to honor the new lease, and vault-format downgrades remain unsupported.
- [ ] Run all automated gates on Linux arm64/amd64; rerun required macOS codec/package CI on final committed code if authority permits. Complete task review and whole-branch review; record remaining native/manual gates explicitly. Commit and offer normal branch integration, not an automatic merge/release.
