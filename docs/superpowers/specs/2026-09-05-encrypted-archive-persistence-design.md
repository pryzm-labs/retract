# Encrypted Archive Persistence — Stage Proposal

**Status:** Proposed; awaiting scope/design approval. No implementation is included.

**Date:** 2026-09-05

**Predecessor:** Provider foundation, merged locally into `main` at `3e9d822`.

**Parent:** [Approved multi-provider architecture](2026-09-01-multi-provider-architecture-design.md), staged delivery item 3.

## Outcome

Provide encrypted local storage and indexed search for normalized archive records. Prove the complete import → search → restart → remove-source lifecycle with synthetic providers before a real Discord or X archive parser is connected.

This is infrastructure for later archive support, not a claim that Discord or X is available to end users. Telegram continues working through its existing live provider bridge.

## Recommended scope and alternatives

1. **Add an archive-only SQLCipher store (recommended).** Introduce content persistence without moving Telegram sessions or replay-sensitive jobs. This isolates the new encryption, indexing, ingestion, and cleanup failure modes.
2. **Move archive content and existing jobs into SQLCipher together.** Eventually reduces persistence implementations, but couples archive work to another destructive-job migration immediately after the foundation's reviewed migration.
3. **Use the existing AES-GCM aggregate for archive content.** Avoids another native dependency but requires rewriting/reading a growing aggregate for indexed operations. It does not meet the approved SQLCipher/FTS design.

**Explicit sequencing amendment for approval:** the parent lists plan/job tables among the future database responsibilities. This stage will not create an unused second job schema or migrate `RTRCT03` jobs. `FoundationStore` remains authoritative for existing plans, jobs, and Telegram identity mappings. Consolidation, if still useful, requires its own later migration design.

## Included

- Source-pinned SQLCipher Community build with FTS5 and tested encryption settings.
- A separately keyed archive database, opened only when archive functionality is requested.
- Versioned schema, source provenance, scoped records, and transactional migrations.
- Bounded normalized ingestion with cancellation, truthful progress, deduplication, and interrupted-run recovery.
- Source-scoped indexed query/resolve operations using the existing neutral records and opaque IDs.
- Local source removal, including dependent search data and recoverable cleanup maintenance.
- Synthetic integration fixtures, failure injection, disk/privacy checks, and build documentation.

## Excluded

- Discord/X ZIP readers, archive HTML/JavaScript execution, media downloads, or real private fixtures.
- Changes to Telegram search/deletion policy, TDLib profiles, or the production UI's current source selection.
- SQLCipher copies of live Telegram message bodies.
- Cross-provider result merging, automatic archive/live identity linking, or additional remote deletion capabilities.
- A new cloud service, paid SQLCipher edition, Apple developer enrollment, or changes to unsigned release policy.
- Production archive storage on platforms without an approved OS credential-store adapter. Linux container tests use injected disposable keys, not a production plaintext-key fallback.

## Architecture and ownership

Keep the implementation in focused modules under `src-tauri/src/persistence/archive/`, with a small import coordinator outside the SQL module. Reuse `retract-domain` identities, scope validation, normalized records, and safe error vocabulary. Do not make the domain crate depend on SQL, Tauri, or a provider.

The coordinator accepts backend-created import sessions and batches through a bounded `ImportSink`; it does not take arbitrary frontend SQL, database paths, keys, or provider locators. Provider adapters validate their own typed identities and locator envelopes before storage. Archive identity is archive evidence, not authenticated live-account authority.

Archive account/source records belong to this store. Existing live Telegram mappings remain in `FoundationStore`; this stage does not dual-write identities or infer links between the stores. The later account-service integration must explicitly reconcile a proven same account while preserving the foundation's stable resource-ID contract.

Database operations execute off the async/UI threads through one serialized writer and a bounded read interface. Hold a process-level store lock so a second app instance cannot race migrations or maintenance. Importing, searching, and deleting use explicit provider/account/source scope. Unscoped convenience methods are not exposed.

## Native dependency gate

Before schema work, select and record the exact SQLCipher source revision, checksum, crypto backend, Rust binding lockfile, compiler flags, and license notices. The gate must demonstrate FTS5 creation/search/deletion, wrong-key failure, and encrypted reopen on Linux arm64/amd64 and the supported macOS package target. A Linux build is not evidence of macOS linkage or Keychain behavior.

Use the existing Docker dependency-fetch/build separation and reusable caches for Linux verification. Native macOS linking/package checks remain on the macOS runner; Docker on macOS does not provide a macOS kernel or SDK. Do not introduce host package-manager prerequisites for end users or claim an untested Windows release.

Failure to build or verify SQLCipher on a required target stops this stage for a design amendment. Ordinary SQLite, plaintext FTS, and an encryption-disabled runtime are not fallbacks.

## Encryption and credentials

Generate an independent archive-index key. Never derive it from an API hash, reuse the TDLib/job key, put it in an environment variable, or return it through IPC.

Extend the consolidated macOS vault with a versioned namespaced representation and `retract/content-index-key`. Preserve the exact existing secret values, legacy read compatibility, synchronized in-process caching, and failed-write rollback. Persist and verify a new key before creating any database that depends on it. Do not re-prompt per database operation. A denied/unavailable vault fails the archive operation without looping or retiring an already-verified Telegram session. This does not promise Telegram can start when its own required secrets are unavailable.

Trigger the vault upgrade lazily on first archive use. An older app that cannot read the upgraded format must fail closed instead of replacing it or dropping the index key. Document that vault-format downgrades are unsupported and test unknown-version preservation; do not promise backward execution compatibility merely because the new reader accepts old vaults.

Key the connection before schema access, perform an authenticated schema read, and verify the required SQLCipher/FTS features before accepting data. A missing key, wrong key, corrupt file, unknown schema, or unexpected plaintext database must not trigger replacement with an empty database. SQLCipher documents that setting a key alone does not validate an existing file. [SQLCipher key verification](https://www.zetetic.net/sqlcipher/sqlcipher-api/#testing-the-key)

Keep file-backed temporary stores disabled and test the main database, WAL, and journal with synthetic canary content. Disable SQL/query-value logging, plaintext exports, and extension loading. These tests complement, rather than replace, authenticated encryption tests. SQLCipher encrypts database and journal pages but requires care around other temporary storage. [SQLCipher temporary-file design](https://www.zetetic.net/sqlcipher/design/)

## Data and provenance

Use versioned schema migrations, archive accounts/sources, a resource-identity catalog, source observations for conversations/actors/content, inert attachment metadata, detector findings/version, import runs, bounded warning summaries, and FTS.

Preserve the foundation's unique `(provider, account, resource kind, canonical key)` identity. Store source observations separately: the same resource can occur in two exports with different text or timestamps without the second import overwriting the first source's evidence. Removing one source must not remove another source's observation or change its IDs.

Foreign-key/scope constraints must cover the whole ownership tuple, not just an individually valid UUID. Native identifiers remain opaque versioned provider payloads. Text, attachment display names, and other sensitive content may exist only inside the encrypted content database and transient query results—not job records, warnings, or diagnostics.

No raw archive copy is retained by default. Persist fingerprints and safe schema identifiers, not absolute archive paths. An archive snapshot never establishes current existence, membership, ownership, or permission to delete remotely.

## Import lifecycle

Start with a backend-only synthetic ingestion path; extend today's inspection-only port with actual sink/session contracts only as required by this lifecycle.

1. Bind a session to a validated provider/account/source and archive fingerprint. Only one writer may import that source at a time.
2. Validate and persist bounded batches with their checkpoints and counters in the same transaction. Batches are limited by both record count and encoded byte size.
3. Keep incomplete sources out of normal search. Publish readiness only after final validation and the readiness transaction commit.
4. Report committed records/bytes and the current phase. Totals are optional; never invent a percentage when the parser has not established a total.
5. Cancellation stops future batches. Roll back an in-flight transaction when it has not committed; retained partial data remains encrypted and explicitly incomplete.
6. After interruption, require an explicit retry/discard. A future parser may resume only after revalidating the same fingerprint, schema, account, and checkpoint. No automatic resume against a different file.

Initial enforced limits are 500 records and 4 MiB encoded data per batch, 1 MiB searchable text per content item, 64 KiB per provider metadata envelope, 100 attachments per item, two queued batches, and 32 distinct warning codes with aggregate counts rather than per-record warning strings. Bound a source to one million content items and 2 GiB cumulative normalized encoded input; database/index overhead is additional and must be measured, not described as part of that 2 GiB cap. Query text is limited to 4 KiB and pages to 200 items. These are backend ceilings, not new environment-variable settings.

Reject oversized values before copying or queueing them. Track cumulative source limits durably, including on retry, and stop safely on storage exhaustion. Include a 100,000-item synthetic corpus plus small limit-injection tests; record memory and on-disk growth. Limit changes require an explicit reviewed update, not silent expansion on failure.

Entry traversal, compression ratios, special files, JSON depth, and archive wrapper validation remain mandatory responsibilities of the later network-isolated parser; this sink alone cannot establish those guarantees.

## Indexed query behavior

Implement source-scoped list/search/resolve operations returning normalized records with archive evidence. Use parameterized SQL and deliberately escaped literal keyword/phrase queries; raw FTS syntax and regex are not new end-user features in this stage.

Use bounded pages ordered by timestamp plus stable content ID. Cursors bind scope, filters, and the published source generation; reject stale/mismatched cursors after source replacement/removal. A source that is still importing, failed, or being removed is not silently returned as a complete result set.

Content rows, FTS updates, findings, and progress checkpoints commit together. Test updates as well as initial inserts so deleted/changed content cannot survive as searchable stale terms.

## Remove a local source

The reviewed effect is **remove this imported copy from Retract**, not remote deletion and not removal of the user's original export file. Provide a backend operation for the later reviewed-action/UI integration; do not expose a new unreviewed destructive IPC shortcut now.

Mark the exact source as removing and invalidate its query generation before deleting its observations, attachments, findings, checkpoints, and FTS entries. Delete identity catalog records only if no surviving source references them. Other accounts/sources and Telegram state are unchanged.

Enable both SQLite core secure-delete and FTS5's separate secure-delete behavior; ordinary row deletion is insufficient to promptly remove old FTS terms. Verify the pinned engine supports this configuration. [SQLite FTS5 secure-delete](https://www.sqlite.org/fts5.html#the_secure_delete_configuration_option)

Checkpoint/truncate the WAL and compact under coordinated exclusive maintenance after logical removal. Maintenance can fail or be interrupted: keep the source hidden, preserve a content-free pending-maintenance record, and report that physical cleanup is incomplete until retry succeeds. Do not re-expose deleted rows, recreate the source, or falsely report full cleanup.

Document that SSD behavior, filesystem snapshots, original exports, and backups can retain data or old encrypted blocks. This action is not a forensic-erasure guarantee.

## Migration and failure behavior

Authenticate before inspecting schema. Refuse newer/unknown versions. For upgrades requiring file replacement, build and validate an encrypted candidate, close/checkpoint connections, and atomically switch only after verification; preserve the original on failure. Do not create plaintext migration dumps or automatically restore older content after an authenticated active database fails.

Return stable, content-free errors for wrong/unavailable keys, corrupt or unsupported stores, scope mismatch, incomplete sources, quota exhaustion, cancelled imports, stale cursors, and pending cleanup. Disk-full/commit failures must not publish counters, readiness, or success that was never durable.

## Acceptance gates

- Required native targets prove SQLCipher/FTS and fail closed with wrong keys, tampering, absent codec, and incompatible schema.
- Vault migration preserves every existing secret and does not introduce per-query Keychain access; failed writes do not replace cached state. Native prompt behavior is a separate manual gate.
- Two synthetic providers/accounts and two overlapping sources prove isolation, stable opaque/large native IDs, deduplication, and independent source removal.
- Actual encrypted-file tests cover interrupted import, commit failure, disk/limit failures, restart, cancellation, stale cursors, and interrupted cleanup.
- Normalized records and search results agree after insert/update/remove; no incomplete source is presented as ready.
- Canary checks cover database/WAL/journal/temp artifacts and captured logs without treating absence of a string alone as proof of encryption.
- A large synthetic corpus demonstrates bounded ingestion/query memory and meaningful progress; no full archive is materialized in memory.
- Existing Telegram characterization, v2 lifecycle, release metadata, bundle, formatting, and Clippy gates remain green.
- README/support claims distinguish implemented storage from deferred real importers and remote remediation.

## Next checkpoint

Review this archive-only scope and the explicit decision to defer job-store consolidation. After approval, write the task-by-task implementation plan, starting with the native dependency gate. This document does not authorize live-account tests, production data migration, or implementation before that approval.
