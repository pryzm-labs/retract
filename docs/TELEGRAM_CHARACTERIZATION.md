# Telegram behavior contract

This document separates the frozen pre-foundation Telegram contract from the current provider-foundation gates. The production Telegram UI now uses scoped version-2 IPC and the authenticated version-3 store through a compatibility bridge to the existing Telegram engine. Automated synthetic parity does not substitute for the separately authorized disposable test-DC matrix in [TEST_PLAN.md](TEST_PLAN.md).

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

- Actual App/desktop-adapter and registered v2 command tests use opaque UUID refs, the distinct native strings `9007199254740992` and `9007199254740993`, signed chat IDs, and a test-only nonnumeric provider. The nonnumeric target traverses the real registry, shared plan/grant/store/execution/recovery path, and scoped reconciliation; it has no alternate test-owned executor.
- Provider/account/source and conversation-aware selection/grouping keys prevent cross-scope collisions. Stale search, polling, settings and per-conversation refresh results cannot replace or clear another context. Dirty refresh resolves only explicit matching-scope conversations, never the full catalog.
- Auth-ready alone is insufficient: a valid authenticated `getMe` and successful durable identity mapping precede usable context. Rename/restart retains account/source identity; test/production, account changes, and ephemeral session generations remain distinct. Delayed/failed lookup and sign-out races fail closed.
- Typed provider locators, canonical refs, frozen recipe/target agreement, ordered effects, confirmation and restart policy bind one SHA-256 fingerprint. One 60-second single-use grant binds that same plan, scope and generation; context is rechecked after the owner prompt and before native calls/after waits.
- Required job creation/progress saves precede mutations or later batches. Save failure cannot publish success, overwrite newer progress or schedule another mutation. Unknown outcomes remain uncertain, not confirmed deletions. Foreign recoverable work is blocked without retargeting or preventing settings changes.
- Backend descriptors control action availability and all ordered reviewed effects; role labels do not grant permission. On-demand job details expose selected/eligible/deleted/skipped/failed/uncertain counters, retries and safe diagnostics. Legacy rows remain read-only with explicit new-review guidance; unavailable historical counts are not invented.
- Production bundle checks exclude screenshot fixtures/reset IPC; the backend synthetic provider is `cfg(test)` only. Automated tests use private temporary profiles and synthetic keys, not real Telegram or Keychain data.

## Current encrypted-store and migration contract

`FoundationStore` is the sole profile writer, shared by an `Arc` in-process and protected by a cooperative interprocess lock. Actual child-process contention tests require `profile_in_use` before any key load, retain the lock until the last owner drops, and allow a subsequent process to reopen. AES-GCM authenticates `RTRCT03` schema/provider/profile binding and validates each account/source/plan/job relationship.

Migration retains the exact authenticated legacy ciphertext at `jobs.pre-provider.enc`, verifies a separate private v3 candidate, then atomically replaces `jobs.enc` and syncs the directory. A different existing backup is never overwritten. Every unfinished legacy job stops and requires a newly reviewed plan; no account is inferred for historical work. The backup is retained, never automatically removed or restored, and is not a general rollback feature. Older readers reject active v3 while the backup remains historically readable.

Tests cover both frozen legacy versions, absent active files with artifacts, interruption before and after replacement, failed candidate verification/write/sync, wrong binding, concurrent transactions, and invalid v3 with a valid legacy backup. Invalid active state fails closed rather than restoring old jobs or creating empty history. Credentials, vault entries and TDLib session/profile paths are unchanged. This stage adds no message-body or private-filename index/persistence; existing exact confirmation titles remain encrypted plan data.

## Live test-DC contract

Use disposable identities and execute the search/catalog, destructive, confirmation/abuse, reliability, and accessibility sections of `docs/TEST_PLAN.md`. Store no account credentials, chat exports, message contents, or TDLib databases in the repository. Record only commit IDs, environment versions, role/capability labels, operation names, provider result codes, counts, and pass/fail outcomes.

## Refactor parity rule

Keep the pre-foundation RED evidence and frozen historical fixtures alongside the passing migrated lifecycle tests. Future removal of the Telegram compatibility bridge must preserve these native semantics and scoped safety checks. A changed result requires an explicitly reviewed behavior change, not an unexplained refactor difference.

## Verification commands and limits

The preferred isolated gate is `npm run container:check`, plus explicit `linux/amd64` and `linux/arm64` checks from [TEST_PLAN.md](TEST_PLAN.md). It runs the full Vitest and Rust suites, TypeScript/build, bundle exclusion, release/public metadata, formatting and Clippy with locked dependencies and non-root/offline project execution.

Native macOS packaging, Keychain/LocalAuthentication, live Telegram parity, VoiceOver and rendered short-viewport modal geometry require separate evidence. The large-plan dialog regression exercises actual CSS and 250 steps in jsdom, not a browser viewport. Report unavailable/manual gates explicitly; do not describe a Linux check as a macOS app build or a live deletion test.
