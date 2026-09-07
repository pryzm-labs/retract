# Telegram behavior contract

This document separates the frozen pre-foundation Telegram contract from the current provider-foundation gates. The production Telegram UI now uses scoped version-2 IPC and the authenticated version-3 store through direct Telegram connection, query and reviewed-cleanup components; the production compatibility bridge and combined cleanup service/gateway have been removed. Automated synthetic parity does not substitute for the separately authorized disposable test-DC matrix in [TEST_PLAN.md](TEST_PLAN.md).

## Preserved Telegram behavior

- TDLib chat, membership, message, media, pin, album, sender, and delete-property values normalize deterministically.
- Main/archive catalogs deduplicate chats and targeted refresh never requires a global catalog reload.
- Global, scoped, empty-query, secret-chat, privacy, date, direction, pin, conversation-kind, and content-kind searches retain their current semantics.
- Plans freeze only the reviewed IDs and expected reach.
- Every frozen message is rechecked immediately before deletion and after a rate-limit wait.
- Cleanup happens before leave; permanent group deletion remains separate.
- Everyone-scoped work never becomes self-only work.
- Broad or ambiguous jobs require new review after restart; only authorized, supported frozen-ID jobs with valid cursors can resume under the original verified account/source.
- Persisted job state remains encrypted, profile-bound, tamper-evident, and content-free.
- Telegram authentication secrets remain absent from settings JSON and frontend snapshots.
- The UI preserves album atomicity, hidden selections, review-before-execute, cancellation, pending removals, duplicate-start rejection, and targeted reconciliation.

## Frozen historical compatibility evidence

The unchanged `telegram-ipc-contract.json` fixture and historical Rust/TypeScript types retain version-1 numeric field/enum/optionality coverage. Those values are deliberately within JavaScript's safe-integer range; they are not the current wire contract or evidence of opaque-ID support. Version-1 destructive handlers are no longer registered in production.

Frozen `RTRCT01`/`RTRCT02` ciphertext fixtures still exercise the historical authenticated reader. The old writer's failed-temporary-write test remains historical coverage, not a description of the current writer or its backup policy. Likewise, the old compact status/retry/deleted row is a historical characterization, not today's complete outcome presentation.

## Current provider-foundation gates

- `TelegramConnection` constructs one native `LiveGateway` with a shared `FoundationStore`, verifies the account/source/session binding, and constructs `TelegramProvider`. Registration returns an independent `TelegramQuery` with only `TelegramRead` plus `EngineContext`, and clones of one `TelegramCleanup` that directly implements `ReviewedLifecycle`. Query objects have no direct cleanup/store access, mutation port or grant book. In production their read port is the shared `LiveGateway`, which retains the identity-persistence store; retaining a query can therefore indirectly prolong the store and profile-lock lifetime without granting store or mutation operations.
- `TelegramCleanup` is the single owner of plans, jobs, cancellation tokens, grants, transition serialization, persistence quarantine and accepted worker lifetime. Planning, authorization, state and execution modules operate on that owner; session-guarded reads and mutations remain separate. Raw remediation registration is unsupported and the retained raw batch method always rejects without consuming authority or scheduling native work.
- TDLib implementation and native DTOs live under the Telegram provider boundary behind separate session, read, mutation and connection-I/O ports. The checked architecture rule runs ten controlled valid/invalid trees and then the real source tree; it rejects the removed facade/service/combined-gateway dependencies and query-to-mutation/cleanup coupling.
- Actual App/desktop-adapter and registered v2 command tests use opaque UUID refs, the distinct native strings `9007199254740992` and `9007199254740993`, signed chat IDs, and a test-only nonnumeric provider. The nonnumeric target traverses the real registry, shared plan/grant/store/execution/recovery path, and scoped reconciliation; it has no alternate test-owned executor.
- Provider/account/source and conversation-aware selection/grouping keys prevent cross-scope collisions. Stale search, polling, settings and per-conversation refresh results cannot replace or clear another context. Dirty refresh resolves only explicit matching-scope conversations, never the full catalog.
- Auth-ready alone is insufficient: a valid authenticated `getMe` and successful durable identity mapping precede usable context. Rename/restart retains account/source identity; test/production, account changes, and ephemeral session generations remain distinct. Delayed/failed lookup and sign-out races fail closed.
- Typed provider locators, canonical refs, frozen recipe/target agreement, ordered effects, confirmation and restart policy bind one SHA-256 fingerprint. One 60-second single-use grant binds that same plan, scope and generation; context is rechecked after the owner prompt and before native calls/after waits.
- Required job creation/progress saves precede mutations or later batches. Save failure cannot publish success, overwrite newer progress or schedule another mutation. Unknown outcomes remain uncertain, not confirmed deletions. Foreign recoverable work is blocked without retargeting or preventing settings changes.
- Backend descriptors control action availability and all ordered reviewed effects; role labels do not grant permission. On-demand job details expose selected/eligible/deleted/skipped/failed/uncertain counters, retries and safe diagnostics. Legacy rows remain read-only with explicit new-review guidance; unavailable historical counts are not invented.
- Production bundle checks exclude screenshot fixtures/reset IPC; the backend synthetic provider is `cfg(test)` only. Automated tests use private temporary profiles and synthetic keys, not real Telegram or Keychain data.

The migration parity corpus in `src-tauri/test-fixtures/telegram-migration/` was captured from the pre-extraction codec at `c2300593fcc0b94c710d6a535173799dd36da4f0`. It contains exact envelopes for all nine native operations and a 36,265-byte authenticated synthetic `RTRCT03` store with SHA-256 `f981b648fdb1bd6b1294d7b2fec53d4794159df00ebe2d9e4fea5ec2b769d07d`. Its manifest freezes UUIDs, scopes, authorization/recovery decisions and fingerprints; tests also reject altered recipes, effects, locators, ciphertext and keys. These fixtures contain no real Telegram data or production secrets and were not regenerated during the provider migration.

## Current encrypted-store and migration contract

`FoundationStore` is the sole profile writer, shared by an `Arc` in-process and protected by a cooperative interprocess lock. The lock guard explicitly unlocks on drop, including failed initialization paths; descriptor clones do not extend ownership. Actual child-process contention tests require `profile_in_use` before any key load, retain the lock until the final store owner drops, and allow a subsequent process to reopen. AES-GCM authenticates `RTRCT03` schema/provider/profile binding and validates each account/source/plan/job relationship.

Migration retains the exact authenticated legacy ciphertext at `jobs.pre-provider.enc`, verifies a separate private v3 candidate, then atomically replaces `jobs.enc` and syncs the directory. A different existing backup is never overwritten. Every unfinished legacy job stops and requires a newly reviewed plan; no account is inferred for historical work. The backup is retained, never automatically removed or restored, and is not a general rollback feature. Older readers reject active v3 while the backup remains historically readable.

Tests cover both frozen legacy versions, absent active files with artifacts, interruption before and after replacement, failed candidate verification/write/sync, wrong binding, concurrent transactions, and invalid v3 with a valid legacy backup. Invalid active state fails closed rather than restoring old jobs or creating empty history. Credentials, vault entries and TDLib session/profile paths are unchanged. This stage adds no message-body or private-filename index/persistence; existing exact confirmation titles remain encrypted plan data.

## Live test-DC contract

Use disposable identities and execute the search/catalog, destructive, confirmation/abuse, reliability, and accessibility sections of `docs/TEST_PLAN.md`. Store no account credentials, chat exports, message contents, or TDLib databases in the repository. Record only commit IDs, environment versions, role/capability labels, operation names, provider result codes, counts, and pass/fail outcomes.

## Refactor parity rule

Keep the pre-foundation RED evidence and frozen historical fixtures alongside the passing direct-provider lifecycle tests. The compatibility bridge removal preserved the same native semantics and scoped safety checks; future ownership changes must continue to do so. A changed result requires an explicitly reviewed behavior change, not an unexplained refactor difference.

## Verification commands and limits

The preferred isolated gate is `npm run container:check`, plus explicit `linux/amd64` and `linux/arm64` checks from [TEST_PLAN.md](TEST_PLAN.md). It runs the full Vitest and Rust suites, TypeScript/build, bundle exclusion, release/public metadata, formatting and Clippy with locked dependencies and non-root/offline project execution.

The migration acceptance baseline at commit `c7f2f5f29387a71c9ac4051b7dd66508baee1311` passed full Linux `arm64` and `amd64` gates with 158 frontend tests, 329 backend tests, 17 cleaner-domain tests, 19 retract-domain tests, 7 release tests, 10 controlled boundary tests plus the real-tree lint, production-bundle/public-repository checks, formatting and strict Clippy. Both builds copied that exact committed tree and executed the project checks; their BuildKit results were `yafbi07vc0a8p6gv3hjryumwt` (`arm64`) and `ado6le3q5vvow7wk1pojcyq9u` (`amd64`). The one ignored backend test is the existing opt-in 100,000-item archive corpus resource gate; it was intentionally not run because this migration does not change archive indexing. Independent whole-branch review of `0d799b0..c7f2f5f` completed with no Critical or Important issues. A prior runtime-only snapshot at `3a78bd207d89be3d3bc93b8cfac6930ce04017e6` remains historical evidence, not the current accepted head. The documentation-only closure after `c7f2f5f` receives its own focused public-repository check and scoped review; no full runtime result is claimed for that later documentation tree.

Native macOS packaging, Keychain/LocalAuthentication, live Telegram parity, VoiceOver and rendered short-viewport modal geometry require separate evidence. The large-plan dialog regression exercises actual CSS and 250 steps in jsdom, not a browser viewport. Report unavailable/manual gates explicitly; do not describe a Linux check as a macOS app build or a live deletion test.
