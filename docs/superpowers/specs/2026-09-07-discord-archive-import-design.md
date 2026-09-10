# Discord archive import

**Date:** 2026-09-07

**Status:** Approved for implementation on 2026-09-07. Format-specific decoding remains gated on an explicitly authorized current structural specimen or official sample.

**Base:** `5dc058016be4400ba70b030ba67ec9a83c94809f` — Telegram migration merged in PR #24.

**Parent:** [Multi-provider architecture](2026-09-01-multi-provider-architecture-design.md), stage 5.

**Preservation contracts:** [Provider foundation](2026-09-03-provider-foundation-design.md), [encrypted archive persistence](2026-09-05-encrypted-archive-persistence-design.md), and [archive storage behavior](../../ARCHIVE_STORAGE.md).

## 1. Decision and boundaries

Build a backend-only Discord Data Package importer that connects a bounded ZIP/schema reader to the existing encrypted archive service. Demonstrate import, indexed search, privacy findings, restart, retry, and local-source removal through integration tests. Discord source selection and remediation UI remain the next separate stage.

The user selected this scope over combining the importer and UI in one larger PR. A parser-only library with no storage integration would be smaller, but would leave identity, recovery, and encrypted indexing unproven; it is not the selected outcome.

Included:

- One explicitly verified current Discord message-export schema profile.
- Defensive ZIP inventory, streaming records, typed Discord normalization and payload validation.
- Scoped identity/source registration, bounded encrypted writes, accurate progress and cancellation.
- Source-scoped search and shared privacy-detector integration using the existing archive query service.
- Synthetic fixtures, failure/recovery tests, resource measurements, and dependency/build checks.

Excluded:

- File-picker/drop UI, new production IPC, source/account switching, and cross-source query merging.
- Discord login, normal-user automation, tokens, API calls, bot moderation, automatic deletion, and browser launching.
- X, historical CSV compatibility, media downloads, attachment-content scanning, OCR, or new privacy classifiers.
- Whole-account analytics, payment data, relationships, sessions, IP history, support tickets, and other non-message export sections.
- Telegram refactoring, TDLib changes, the deferred privacy-scan diagnostic investigation, or existing job-store migration.
- A public release, upload of private exports, or claims of forensic erasure.

Discord explicitly prohibits normal-user self-bot automation. This importer has no such functionality. [Discord self-bot policy](https://support.discord.com/hc/en-us/articles/115002192352-Automated-User-Accounts-Self-Bots)

## 2. Format evidence is the first implementation gate

Discord currently describes a ZIP with account metadata, per-channel context and JSON message transcripts, including IDs, timestamps, text and attachment links. The message section represents sent messages, not a complete transcript of both sides. [Official Data Package description](https://support.discord.com/hc/en-us/articles/360004957991-Your-Discord-Data-Package)

The documentation checked on 2026-09-07 is not a machine-readable schema: it does not establish exact filenames, JSON root shapes, field capitalization/types, optional/null variants, timestamp grammar, or attachment encoding. This design deliberately does not invent those details or treat old CSV examples as current JSON evidence.

Before implementing the Discord decoder:

1. Obtain a current, explicitly authorized structural specimen from a disposable account export, or an official sample that supplies equivalent evidence. A user may instead supply a locally sanitized structure-only specimen. Do not search the user's disk for private archives or upload an export anywhere.
2. Record paths, wrappers, field names/types, timestamp forms, ID representation, and attachment representation. Cover direct messages, group DMs, server channels, empty messages/attachment-only records, and empty exports. Only claim supported variants that have evidence.
3. Hand-author a minimal synthetic package reproducing those structures. Replace all identities, text, names and URLs; never copy the real account file or unrelated export sections. Document provenance, observation date, supported variants, synthetic-generation method, and fixture hashes.
4. Freeze a profile manifest and normalization expectations under `src-tauri/test-fixtures/discord-import/`. Give the profile a backend-owned version and pin its interpretation for deterministic retries.

This is a hard dependency gate, not an invitation to guess a parser. No current structural specimen has been inspected for this proposal. If one cannot be obtained, report the missing evidence and stop the format-specific implementation. No credentials or private message bodies are needed for this gate.

The first implementation supports that single current JSON profile. CSV, multiple roots, and different wrappers return an unsupported-profile error; they need separate fixtures and review before support is added. Extra fields within an accepted profile can be skipped with bounded parsing; changed required fields, ambiguous roots, duplicate required JSON keys, or mixed supported/unsupported transcript formats fail closed.

### Task 1 dependency and preflight evidence (2026-09-09)

The format gate remains closed: no real/private specimen has been inspected. The authorized pre-gate work is a format-neutral bounded ZIP inventory and value-free structure probe. `zip` is pinned to MIT-licensed 8.6.0 with defaults disabled and only `deflate-flate2-zlib-rs`; policy permits stored/deflate and rejects encryption and other methods. Its resolved checksum is `2d04a6b5381502aa6087c94c669499eb1602eb9c5e8198e534de571f7154809b`; the codec checksums are `flate2` 1.1.10 `6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb` and `zlib-rs` 0.6.7 `34b31d188d9d685a4f9c7b46d6e36631b07058d2cfe190267adce54dc230bf12`. The complete ZIP-subtree checksum/review record is in [third-party notices](../../../THIRD_PARTY_NOTICES.md), and all resolved parser package checksums are in [the Cargo lockfile](../../../crates/discord-archive/Cargo.lock).

Source review established that `ZipArchive::new` allocates its central index eagerly, ignores local headers and can retry older EOCD candidates. Preflight bounds directory/count/all declared metadata first. Library indexing then uses a cached metadata view with one EOCD and no payload/comment end records; embedded directory end markers are rejected. Independent local-header/range checks compensate for the library's partial local checks. Consumed entries must reach CRC-checked EOF with exact observed sizes before validation. Unselected payloads remain uncertified.

Explicit Task 1 rulings keep the exact dependency surface and fail closed: entry paths must be valid ASCII after UTF-8 validation; non-ASCII, case collisions and nonportable dot/space suffixes return `UnsafeEntryName`. All ZIP64 forms return `UnsupportedZip64`; data descriptors and unsupported metadata also fail closed. The later evidence task must request an amendment if official packages need these variants. The probe supplements the named design ceilings with lowerable limits of 1,000,000 JSON tokens and 32 MiB accounted retained structure across selected entries. Only map-key names, templated paths, shape/type information and occurrence counts are reportable; string/number values and raw bytes have no report representation. JSON keys can themselves contain identifiers in arbitrary data, so the report remains a private evidence artifact requiring review before sharing.

The pre-profile evidence amendment adds closed grammar tokens to each JSON node, with separate sets for decimal strings, original numeric tokens and timestamp strings. An absent scalar kind is `NoObservation`; observed unknown timestamps are `Unclassified`. Decimal evidence distinguishes canonical positive ASCII decimal integers within `u64`, exactly zero, noncanonical numeric syntax, canonical decimal overflow and other strings. A constant-space scanner classifies the original numeric token before Serde conversion; rounding, negative zero or exponent notation cannot turn into canonical-ID evidence. String signs, leading zeros, ASCII whitespace, decimal points and exponents remain noncanonical; malformed numeric strings are `Other`, and malformed JSON still rejects the entire report. Sorted, deduplicated enum sets retain no examples, scalar lengths, hashes, raw offsets, unknown literals or dynamically discovered labels. Their headers and observed members are charged against the existing retained-structure budget before allocation.

Timestamp evidence has a finite vocabulary: Gregorian years 0001–9999, `T` or space separator, seconds/milliseconds/microseconds/nanoseconds, and uppercase `Z`, colon-delimited numeric offset or unzoned form. It validates calendar/time/offset ranges and rejects unknown precision, lowercase/unknown suffixes, extra whitespace, invalid dates, leap seconds and negative-zero offset as `Unclassified`. Precision is a named grammar alternative; dates, times, fractional digits and offset values are never retained. `Unzoned` supplies no timezone assumption. Path evidence recognizes canonical positive `u64` decimal segments and one additional predeclared token for lowercase `c` followed by that grammar; other mixed segments remain unknown, and the existing literal allowlist is unchanged. These generic categories establish no supported Discord profile or context/attachment interpretation. A reviewed amendment and a separately authorized probe run must precede freezing any such profile; the ZIP policies and all resource ceilings remain unchanged.

The subsequent path-candidate amendment predeclares three exact literals as evidence hypotheses: `Account` maps to `TitleCaseAccount`, `Messages` to `TitleCaseMessages`, and `user.json` to `UserJson`. Lowercase root tokens remain distinct. Matching is per segment and case-sensitive, without case folding, prefix matching, dynamic discovery or raw-literal output. All other spellings and unknown paths retain their existing redacted categories. Synthetic exact-case, lookalike and JSON/Debug non-disclosure tests constrain this vocabulary. These tokens do not authorize or establish a supported profile; independent review and complete authorized evidence remain prerequisites.

## 3. Component boundaries

```text
Backend-owned import coordinator
  ├─ read-only selected file + cancellation + progress
  ├─ isolated Discord reader → bounded typed records
  ├─ Discord normalizer / payload validator
  └─ ArchiveOwner → ArchiveService → encrypted ArchiveStore
                                     ├─ indexed query
                                     └─ existing privacy detector
```

### Reader

Add `crates/discord-archive/` with focused ZIP-policy, schema-profile, record, limit, and diagnostic modules. It receives a `Read + Seek` input and a backpressured record sink, not paths to open, an HTTP client, Tauri state, database connections, credentials, or a Telegram gateway. It emits bounded typed Discord records and content-free counters/error codes; it does not allocate Retract UUIDs or make storage decisions.

Select and pin a ZIP dependency with only required stored/deflate support. Review central-directory allocation behavior, ZIP64 handling, local-header validation, integrity checks, licenses and transitive features before adoption. Do not enable encryption, exotic codecs, remote I/O or lifecycle scripts by default. Add crate checks to both Docker architectures and the native macOS build workflow.

The crate boundary and dependency checks prevent accidental coupling; they are not an OS sandbox against arbitrary code execution. The parser runs in a tracked blocking task within the application process. Docker isolates build/test execution, not the installed app's runtime. A separate sandboxed parser process is a later hardening option, not a guarantee of this stage.

### Discord adapter

Add focused modules under `src-tauri/src/providers/discord/` for typed versioned locators, normalized records, archive payload validation, import coordination and tests. Reuse `retract-domain` records and stable `ProviderResourceRef::resource_id()` derivation. No decoder detail belongs in generic query/UI code or in the database module.

Implement all archive validation hooks in `ProviderPayloadValidator`; validate nested participant and attachment envelopes too. Reject remediation recipes because this stage supports no executable Discord action. The production lazy archive opener must register the Discord validator so it can reopen valid Discord observations; registration must not open the database or touch Keychain at startup.

### Coordinator

The coordinator owns file validation, hashing, session/worker lifetime, deterministic batching and calls to the existing `ArchiveService`. It cannot accept frontend-supplied SQL, account/source UUIDs, keys, trusted locators, or mutation sessions. No production IPC is added in this stage.

Keep the existing inspection-only `ImportInspector` unchanged for now: its required item count cannot represent a cheap ZIP inventory with an unknown message total. Use a backend-only inspection/progress type with optional totals. The later UI stage can adapt the public contract explicitly without pretending unknown totals are zero.

## 4. ZIP and parser policy

Open only an explicitly supplied regular file in read-only mode, rejecting a final symlink and special files. Retain that file handle through inspection/import; do not reopen entry paths. Never extract, execute, render HTML, evaluate JavaScript, recursively open embedded archives, or fetch URLs.

Preflight the end-of-directory structures before calling any ZIP API that eagerly allocates an entry index. Bound directory bytes and entry count; check ranges with checked arithmetic. Reject multi-disk, encrypted, self-extracting/prefixed, unsupported compression and contradictory ZIP64/local-header metadata. ZIP64 is allowed only when the selected library and policy tests establish the same bounds.

Validate all entry names and declared sizes, including unselected entries. Reject absolute paths, traversal, backslashes, drive/UNC paths, NUL/control characters, invalid UTF-8 names, duplicate or normalization/case-fold-colliding names, file/directory conflicts, symlinks and special-file metadata. Accept only ordinary files/directories with supported metadata; no filesystem extraction means no archive-created hard links. Directory records must carry no file payload.

Check overlapping/out-of-file ranges and local/central agreement for consumed entries. Count observed decompressed bytes, enforce per-entry and aggregate quotas, and finish CRC/integrity checks before marking an entry validated. Batches may commit while an entry is streaming, but remain hidden; a late CRC error fails the source instead of requiring the entire transcript to be buffered. Unrelated sections are not decompressed; readiness does not certify their payload CRCs. Their declared sizes still count against the package-wide ceiling.

Proposed initial named ceilings, subject to an explicit design amendment if the format gate proves them incompatible:

| Resource | Ceiling |
| --- | --- |
| ZIP file bytes | 4 GiB |
| Central-directory bytes / entries | 32 MiB / 100,000 |
| Entry path bytes / components | 1,024 / 16 |
| Declared total expanded bytes, all entries | 16 GiB |
| Expanded bytes per consumed entry | 2 GiB |
| Observed expanded bytes per parsing pass | 4 GiB |
| Expanded/compressed ratio per file | 1,000:1; nonempty output with zero compressed length rejected |
| JSON nesting / decoded scalar bytes | 64 / 1 MiB |
| One raw / decoded message record | 8 MiB / 2 MiB |
| Account/channel display string | 4 KiB |
| Selected channel contexts | 40,000 |

Retain existing storage bounds: 500 batch records including embedded participants, 4 MiB encoded batch, two queued batches, 1 MiB searchable text, 64 KiB provider envelope, 100 attachments/item, one million distinct content items and 2 GiB accepted normalized input per source. Reject rather than silently truncate. Raw input limits and database/index overhead are separate from the normalized-input quota.

Use streaming JSON-array/object visitors with byte/token/depth limits enforced before unbounded allocation, including ignored values. Do not deserialize an entire transcript or account object into a generic JSON value. Consume only required account identity/display fields; discard unrelated account fields without retaining them. Inventory metadata is bounded by the directory budget; channel context is processed one channel at a time, not retained for the entire archive.

Check cancellation between bounded reads, records and batches. Report time spent hashing/validating rather than showing a stationary import count. A corrupt or unsupported message entry fails the source; it is never silently omitted from an apparently complete import.

## 5. Identity, normalization and evidence

Only a validated export account ID binds the import. Missing or contradictory identity fails with a safe error; do not infer identity from a username, filename, participants, or the connected Telegram account. Archive identity is a claim made by the package, not authenticated Discord login or ownership proof.

Keep Discord IDs as lossless decimal strings. A JSON integer is accepted only if its original token can be validated losslessly; floats/exponents, negative values and noncanonical forms are rejected. Never pass through JavaScript numbers. Require IDs to fit the profile's documented ID grammar; fixtures must include values above JavaScript's safe-integer range.

Use these provider-owned resource namespaces inside versioned locator envelopes:

- Account/actor: Discord user ID.
- Conversation: channel ID; optional guild context belongs in provider metadata, not in a mutable canonical key.
- Content: channel ID plus message ID.

Resource locators include stable identifying fields only. Do not put display names, attachment URLs, optional guild enrichment or timestamps in a resource locator: the store requires an existing resource's locator to remain identical across snapshots. Store those attributes in source-specific observation metadata instead.

Resolve account identity through the archive store's existing unique provider/canonical-identity constraint, allocating an internal account ID only when absent. Do not create a second UUID for the same account or dual-write into Telegram's FoundationStore. Content/actor/conversation IDs then follow existing deterministic resource-ID derivation within that account.

Map verified DM/group-DM/channel context to the existing conversation kinds; retain guild/channel names and IDs as typed metadata. Unknown or absent type information stays `Other` with a bounded warning. Do not infer group membership, current permission, or a complete participant list. Do not synthesize a guild as a message conversation. Thread/reply relationships are populated only when the verified profile supplies them; otherwise leave them absent.

Attribute sent records to the export account; if the profile explicitly supplies a contradictory author, fail. Preserve Unicode, multiline text and attachment-only messages. Accept only the profile's specified timestamp forms, normalize explicit time zones to UTC, and never use the host's local timezone or current time as a substitute. Missing/invalid required timestamps or IDs fail the source.

Attachments are inert metadata. Retain bounded provider references inside encrypted envelopes; never download them. Validate URL syntax and reject active/local schemes. Unknown media types remain `Other`; filename-derived hints are not proof of MIME type. No claim is made that a link remains valid or a file's contents were scanned. Do not store full raw export records merely to preserve optional fields.

Every record has archive evidence and the account remains disconnected. `external_location` stays unsupported in this stage; storing native IDs prepares the later reviewed external-location feature without enabling it now. The existing store derives privacy findings from normalized text and attachment display names, using the existing detector/version.

## 6. Source identity, duplicates and snapshots

Hash the exact selected ZIP bytes with SHA-256 in bounded chunks before registration. The import identity is provider + canonical account + archive fingerprint + schema/parser policy version. The hash is local provenance, not proof of Discord authenticity, and is never logged alongside message data.

Add an atomic backend get-or-register operation backed by indexed archive identity lookup:

- An identical ready import returns the existing source/checkpoint, without new observations or another parse/write run.
- An identical active import returns busy/status, not a second writer.
- An identical interrupted/cancelled/failed import requires explicit retry or discard.
- A different fingerprint creates a new source snapshot for the same account. Stable resource IDs are reused, but observations remain source-scoped.

This stage does not merge snapshots into an all-source view or delete older sources automatically. An absent message in a newer archive never means remote deletion. Same content across snapshots shares identity; repeating an identical archive does not create another source. Removing one snapshot preserves every other source's observations. A deliberately removed source UUID is never resurrected; reimport after removal allocates a new source UUID.

For duplicate message IDs within one package, reject conflicting normalized observations. Compare provider-normalized content, excluding store-derived detector fields. Exact duplicates may be counted and ignored deterministically. Detection must be bounded and include duplicates crossing batches; use the encrypted source observations/receipts rather than an unbounded in-memory ID map. Emit duplicates into deterministic batches and enforce conflict checks/suppression inside the transaction; do not change parser output based on which rows are already stored during retry. Check within-batch conflicts too, before the store's existing duplicate suppression. Do not allow ordinary upsert behavior to silently choose the last conflicting record.

## 7. Import, cancellation and recovery

Run one parser job at a time per application in this first stage, with at most two queued batches through the existing writer. Own its completion handle before launching it. Application shutdown rejects new imports, signals cancellation, drains parser/coordinator work, then drains the archive owner before clearing credentials. Dropping a request waiter must not detach the parser or allow a late write after shutdown.

The operation has explicit phases:

1. **Inspecting/hashing:** validate file and ZIP inventory, determine profile/account and compute fingerprint. Do not open the encrypted store or request its key for an obviously unsupported file.
2. **Registering:** open the archive owner lazily and resolve exact account/source identity.
3. **Importing:** parse channels in a deterministic inventory order, emit the author/context before their messages, normalize records, and submit deterministic bounded batches. Progress separates scanned records/bytes from acknowledged committed unique items/bytes.
4. **Verifying:** finish required-entry parsing and integrity checks; recheck the file identity/size/metadata and full fingerprint using the retained handle before publishing readiness. Any detected input change fails the import, keeping partial data hidden.
5. **Ready:** only after final validation and the store's readiness transaction succeeds.

Retaining a handle plus before/after checks detects ordinary file modification/replacement; it is not protection against a malicious same-user process racing and restoring bytes. No plaintext snapshot copy is made. The original ZIP is never altered or deleted, and its absolute path is not persisted.

Expose backend progress as latest-value/coalesced state, not an unbounded event queue. Include phase, inventory/processed entry counts, bytes hashed/read, parsed records, committed unique items/bytes and warning counts. Totals are optional; entry/byte progress is not a fabricated message percentage. During retry, committed counts may already be nonzero while deterministic replay catches up.

Reuse the existing explicit retry authority. The first implementation reparses the same file from the beginning and replays the same numbered batches against existing digests; it does not seek to arbitrary compressed offsets or skip data using a user-supplied checkpoint. Freeze parser policy, ordering, batch-boundary rules and observation time at initial registration. Reuse those on retry—never call `now()` per replayed record. Exact replay does not charge committed quotas twice. A changed file, account, profile, normalization policy or batch digest is not resumable.

Cancellation sets the shared signal immediately, including while waiting for a queue slot, then persists the cancelled phase. Already committed rows remain encrypted and unsearchable until a successful explicit retry. Parser errors persist a fixed failure code and failed state through the coordinator. If storage is unavailable and that state cannot commit, report the storage failure; reopening must leave the run interrupted, never ready. If completion already committed before a late cancellation, return the actual ready state rather than claiming the successful import was rolled back.

Invalid required data fails the entire source. Warnings are only for supported, nonfatal omissions such as unavailable optional context; the importer must not label skipped invalid messages a complete import. Discard uses the existing local-source removal/maintenance backend and affects neither the original ZIP nor remote Discord content. No new unreviewed destructive IPC is introduced.

## 8. Narrow storage extensions

The current store already supplies session authority, transactional batch receipts, hidden incomplete sources, privacy findings, query pagination and removal. It does not yet supply atomic canonical-account/import discovery, parser warning submission, or an explicit parser-failure transition on the worker port.

Add only these required capabilities:

- Indexed atomic account/import lookup and registration, with scope validation and source-removal cleanup.
- Fixed-code, bounded parser-warning deltas committed with their batch/receipt. At most 32 distinct warning codes; no arbitrary warning strings, paths, snippets or URLs. Replaying a receipt must not double-count warnings; finalization must preserve them alongside existing missing-reference warnings.
- Session-checked failure completion, preserving acknowledged counts and safe failure kind.
- Immutable run observation time, reused on replay instead of deriving it from mutable source timestamps.
- Duplicate-observation conflict checking for this importer without changing another adapter's ordinary upsert semantics.

Use an explicit archive schema v2 migration for the new lookup/receipt/diagnostic storage; never silently modify the v1 DDL hash. Preserve existing accounts, sources, observations, receipts, FTS and removal tombstones. Keep legacy zero-warning receipt digests valid; version the new receipt payload when warning data contributes to its digest. Only the Discord import path opts into the new import-identity mapping. Do not reinterpret old synthetic or other-provider source provenance as Discord data.

Migration tests must start from a frozen authenticated synthetic v1 artifact, not an artifact generated by the new schema code. Follow the existing encrypted migration and failure-preservation policy; unknown versions fail closed. Do not combine this with job-store consolidation, credential format changes, new keys or plaintext staging. If these additions require broader persistence changes than described, return for a design amendment.

## 9. Validation and acceptance

Before integration is called complete:

1. **Schema evidence:** a provenance manifest establishes the supported current JSON profile and every advertised variant; no guessed CSV/JSON compatibility claim.
2. **Parser/security:** tests cover malformed/truncated ZIPs, central-directory allocation bounds, local-header disagreement, duplicate/ambiguous paths, traversal, special entries, encryption, bad CRC, overlap/range overflow, ZIP64 limits, compression bombs, invalid UTF-8, deep/large JSON, duplicate required keys, mixed/unknown formats and input changes. Limits are checked on observed bytes, not just archive declarations.
3. **Normalization:** opaque large IDs, account/channel mismatches, DM/group/server contexts, missing optional metadata, exact Unicode/text, empty/attachment-only records, timestamps, unknown media, inert URLs and duplicate conflicts have synthetic expectations.
4. **Encrypted integration:** import → complete paged query → privacy findings → restart → repeat import → newer snapshot → remove one source. Confirm stable IDs, exact per-source observations, unchanged Telegram state and no private content in DB sidecars, temporary files or diagnostic output outside the encrypted store.
5. **Failure/lifetime:** test full queues, cancelled waiters, cancellation during hashing/parsing/commit, parser errors after committed batches, disk-full/finalization failures, interrupted reopen, exact receipt replay, changed-policy/file refusal, same-source races and shutdown ordering. No failure publishes a ready incomplete source or leaves an unowned worker.
6. **Migration:** frozen v1 archive, successful v2 reopen, invalid/newer schema rejection, interrupted upgrade preservation and warning/receipt compatibility.
7. **Resources:** run 100,000 synthetic messages through the actual ZIP reader/coordinator/store, measure whole-process peak RSS and DB/temp disk growth, and compare against a smaller corpus to expose whole-transcript materialization. Report raw ZIP, decoded input and normalized/database sizes separately. No general claim of million-message performance follows from this test. Small injected-limit tests exercise every production ceiling.
8. **Regression/build:** full existing frontend/domain/backend, strict Clippy/fmt, provider-boundary and public-repo checks in Linux ARM64/AMD64 Docker; add parser tests to both gates and the native macOS package/test gate. Keep dependency fetch separate from offline project execution, use shared named caches, avoid image exports or cache pruning.

Extend architecture checks to reject parser dependencies on networking, credentials, Telegram, UI and persistence; test the check itself with controlled violations. This is defense against accidental dependency drift, not proof of an OS sandbox. Runtime tests must show no URL opening/fetching and no new mutation registration.

## 10. Delivery and handoff

The implementation plan will break this into reviewable commits: schema-evidence/dependency gate, isolated ZIP reader, Discord codecs/validator, narrow persistence migration, coordinator/lifetime integration, then regression/resource/native acceptance and documentation.

Likely changes are confined to the new parser crate and Discord provider, archive store/worker/model/migration hooks, parser ownership at the application boundary, synthetic fixtures, Docker/CI checks and documentation. The frontend and Telegram provider algorithms are unchanged. The production application must not advertise usable Discord support until the following UI stage is implemented and tested.

After this specification is approved, write the detailed implementation plan. Passing that plan's format-evidence gate is required before parser implementation. UI integration, real private-export testing, publication and merging remain separate decisions.
