# Telegram provider migration

**Date:** 2026-09-06

**Status:** Written specification approved for implementation on 2026-09-06.

**Base:** `0d799b0a272373601714fd1c0645fbe618fbb870` — merged PR #22.

**Parent:** [Multi-provider architecture](2026-09-01-multi-provider-architecture-design.md), stage 4.

**Preservation contracts:** [Provider foundation](2026-09-03-provider-foundation-design.md), [Telegram characterization](../../TELEGRAM_CHARACTERIZATION.md), and [archive storage](../../ARCHIVE_STORAGE.md).

## 1. Decision

Replace the production Telegram compatibility facade with focused, Telegram-owned connection, query, and reviewed-cleanup components. Extract the existing behavior; do not rewrite TDLib or change which data an action deletes. Fix the known FoundationStore lock-ownership defect first as a separate, test-first prerequisite commit.

The user approved this scope and requested implementation. This document records the detailed compatibility and acceptance requirements for the architectural review gate; it does not claim implementation has begun.

### Alternatives

- **Incremental extraction — selected:** retain tested native algorithms and persisted recipes while removing the combined service and gateway dependencies from production composition. Each responsibility can be verified independently.
- **Full neutral-engine rewrite:** replace Telegram's compound executor with the generic frozen-item lifecycle now. Rejected because that lifecycle does not represent all broad and cleanup-before-leave phases; extending it and changing execution together would add avoidable risk.
- **File relocation only:** move the current classes under a Telegram folder. Rejected because query operations would still depend on cleanup state and every consumer would still receive the combined gateway.

### Included

- Focused native connection, read/query, and mutation interfaces inside the Telegram provider.
- Direct implementations of the existing application query, connection, and reviewed-lifecycle ports using those interfaces.
- Provider-owned planning, normalization, recipe validation, native failure classification, and compound execution.
- Removal of the production `CleanerService` / `TelegramCompatibilityProvider` delegation chain and combined `TelegramGateway` dependency.
- Stable v2 IPC and v3 persisted plan/job behavior, with before/after synthetic evidence.
- Explicit FoundationStore lock release when the final store owner drops.

### Excluded

- Discord/X integration, archive import UI, mixed-source search, multi-account UI, new deletion modes, and UI restyling.
- TDLib version changes, catalog/search algorithm rewrites, new pagination semantics, and Telegram message-body indexing.
- Changes to credentials, Keychain prompts, TDLib directories, encrypted archive schema, job-store format, or encryption keys.
- New public raw-mutation IPC, an execution bypass, automatic retries of ambiguous work, or stronger deletion-verification claims.
- Publishing, merging, tagging, or releasing this next stage without the normal handoff decision.

## 2. Current implementation and migration boundary

`ProviderService` already validates v2 requests and dispatches through `ApplicationConnection`, `ApplicationQuery`, and `ReviewedLifecycle`. It is not necessary to migrate the frontend to another wire version.

The remaining dependency is inside Telegram: `TelegramCompatibilityProvider` owns both a combined `TelegramGateway` and `CleanerService`; the service owns search, refresh, planning, grants, persistence, and jobs. `TelegramConnection` constructs this pair after verified identity is available. `LiveGateway` and `TdJsonClient` retain the characterized TDLib behavior. The generic `FrozenLifecycle` and Telegram executor already share `GrantBook`, `ScopedRepository`, and `run_frozen_batches`.

The target keeps one TDLib runtime and one cleanup-state owner per active Telegram context. It does not construct competing connections, duplicate caches, or open a second FoundationStore writer.

| Component | Responsibility and dependencies |
| --- | --- |
| `telegram/connection.rs` | Typed auth/bootstrap, identity establishment, shutdown, and provider construction. Implements `ApplicationConnection`; owns native connection lifecycle. |
| `telegram/query.rs` | Implements `ApplicationQuery` and `QuerySource`; validates locators/filters, performs read-only lookups, normalizes results, and refreshes only explicit chats. No job store, grant book, or mutation handle. |
| `telegram/remediation/` | Implements `ReviewedLifecycle`; discovers actions, freezes plans, authorizes, executes, cancels, recovers, and reports jobs. Owns the only mutable cleanup runtime. |
| `telegram/remediation/planning.rs` | Existing selected, own, sender, and conversation planning rules and action/effect descriptions. |
| `telegram/remediation/execution.rs` | Native compound-operation ordering, live rechecks, retry policy, and callbacks to the shared frozen-batch loop. |
| `telegram/remediation/state.rs` | Serialized transitions, durable publication, grant ownership, worker tracking, and failure quarantine. |
| `telegram/recipe.rs` | Pure content-free persisted recipe codec, envelope/fingerprint validation, and job projection. No service or gateway construction. |
| `telegram/normalize.rs` | Pure native-to-normalized records, metadata, and action presentation helpers. |
| `telegram/native/` | TDLib runtime/client and focused native ports. Preserves the existing raw request mapping, caches, timeouts, and update handling. |
| Existing neutral provider modules | Scope validation, registration, frozen-batch orchestration, grants, and scoped repository publication. No Telegram-native policy branches. |

The filenames define ownership, not a requirement for an artificial one-file-per-method split. Related code may stay together when dependencies remain focused. Do not replace the old combined object with a newly named combined object.

Move Telegram-native request/job/auth types under the provider. Historical v1 fixtures and legacy store readers may retain explicitly named compatibility types; they are read-only/test boundaries, not a second production service. `cleaner-domain` remains the tested Telegram policy library in this stage, not a domain to use for future providers. Its deletion algorithms are reused from Telegram modules rather than mechanically generalized.

Production shared services must not import the old service/gateway, decode Telegram locator fields, or branch on Telegram action names. Application composition and typed Telegram setup are explicit integration points, not generic dispatch logic. Shared infrastructure errors may remain shared, but Telegram error parsing and native-to-safe translation belong to the adapter. Do not change current wire error codes under the guise of relocation.

## 3. Focused native ports and runtime ownership

Split the current native gateway by the operations consumers actually require:

- **Session view / connection:** verified identity, runtime mode, auth and catalog progress; typed sign-in operations and close belong only to connection ownership.
- **Query:** catalog enumeration, one-chat lookup, filtered search, own/all history, exact-message resolution, sender lookup, and current deletion reach. These operations cannot mutate Telegram.
- **Mutation:** the existing explicit native operations for everyone deletion, clear-with/without-retaining-chat, local removal, group destruction, leaving, and sender deletion. No generic boolean-driven `delete` replacement.

Production query objects receive only the session view and read port. Cleanup receives the session view, read port for preflight, mutation port, and scoped state repository. Connection creates these views over the same immutable native session; it does not reopen TDLib to create a port.

Scope/generation checks remain before and after awaited provider work, after device-owner prompts, before native mutations, and after retry waits. The session guard must protect each focused port, not disappear when the combined `SessionGateway` is split. Native requests continue using the captured session; no handle can be rebound to another account behind an active worker.

Keep the existing `ApplicationConnection` and `ReviewedLifecycle` as the production control plane. The generic `LiveConnection` and raw `RemediationProvider` methods are not new user-facing features in this migration. Do not register a raw execution port merely to satisfy a trait inventory: an absent/unsupported port remains unsupported, and direct batch calls cannot authorize native I/O. Registration capability declarations must remain consistent with what is genuinely exposed.

## 4. Query parity and isolation

Retain the v2 request/response field names, optionals, safe errors, and normalized record shapes. Preserve Telegram filter metadata schema/version and all existing direction/date/content/chat-kind/pin/privacy semantics.

Move request validation, conservative truncation behavior, and targeted refresh from the old service into the query component. Refresh keeps the existing maximum of 1,000 IDs, deduplication, eight concurrent lookups, ten-second per-chat timeout, title sorting, and omission of missing chats. An action-preparation or refresh lookup must not enumerate the global catalog.

Keep current `QuerySource` cursor behavior: unsupported cursors are rejected, not ignored or advertised as usable. Telegram's internal TDLib paging remains unchanged. This stage does not claim new user-visible pagination or completeness guarantees.

Bootstrap continues reporting measurable identity/catalog progress without starting another catalog scan. No query needs a grant, a cleanup-state write, or an archive-store open. Shutdown/settings replacement preserves the existing service lease, worker drain, archive-owner drain, and credential-clear ordering.

## 5. Cleanup, authorization, and persisted compatibility

The new Telegram cleanup component implements the complete reviewed lifecycle directly. Extract planning and execution from `CleanerService`, sharing the current neutral primitives. Do not route all compound operations through `FrozenLifecycle` or create a second frozen-batch loop.

Preserve every existing operation: selected messages, own messages without leaving, whole-history revoke, self-only chat removal, own/admin-wide frozen cleanup before leave, broad clear-history before leave, sender cleanup, and standalone permanent group destruction.

Preserve the existing action IDs, ordered effect descriptors, confirmation tiers, exact-title checks, prompt sanitization, and one 60-second single-use grant for the same scoped fingerprint. Role labels and static capabilities never authorize work. The frontend never supplies an executable recipe or native permissions.

Execution preserves:

1. Frozen IDs and the existing 100-message, single-chat batch grouping.
2. Per-message reach checks and cancellation checks before mutation and after waits.
3. Authority rechecks for broad operations; no downgrade from everyone to self-only cleanup.
4. Cleanup before leave; acknowledged compound-step saves before later mutations.
5. Duplicate-start rejection, existing retry bounds/deadlines, durable progress before cursor advance, and quarantine on required save failure.
6. Truthful skipped/failed/uncertain reporting and current ambiguous/restart rules. Server acceptance is not newly described as independent deletion verification.
7. Targeted dirty-reference reconciliation only for the original scope.

### Saved-state compatibility

Keep `RTRCT03`, locator schemas, UUID derivation, metadata schemas, plan canonicalization, and `telegram.compatibility_recipe` version 1 unchanged. The persisted schema name is a historical identifier, not a runtime architecture requirement; do not rename it to match new Rust modules.

Extract pure recipe binding/validation from the facade so neither the codec nor repository needs a provider/service instance. The recipe codec must produce byte-equivalent canonical fingerprint inputs and identical envelopes for the same native plan. Plan/job UUIDs, counters, timestamps, retry state, authorization provenance, cursors, safe diagnostics, and dirty references survive loading unchanged except for already-defined recovery transitions.

Retain the existing guarded v1/v2 historical readers, v3 writer, scoped compare-and-swap publication, and foreign-scope blocking. No schema upgrade, backup rewrite, new key read, or resealing of old fingerprints is needed. If extraction cannot preserve this contract, stop for a design amendment rather than silently invalidating plans or loosening validation.

## 6. FoundationStore lock prerequisite

The current `_profile_lock: File` relies on closing its descriptor. A retained clone/inherited open-file description can keep the lock alive after the store is gone, causing conservative `ProfileInUse` failures during immediate reopen. The archive store already uses explicit unlock ownership.

Introduce a private profile-lock guard whose destructor calls `File::unlock()` before closing the file. Retain the guard as the final store field so state and write-capable resources drop first. Only the final `Arc<FoundationStore>` owner releases it; do not add an early public unlock method, remove the lock file, retry around contention, or weaken the before-key-load lock acquisition rule.

Start with a deterministic failing test that retains a clone of the lock's file description: an independent writer's lock acquisition must fail while the owner exists, dropping a nonfinal store reference must not unlock, and a fresh store must open after the final owner drops even while the retained file description exists. Registered same-process opens still reuse the existing `Arc`; they are not competing writers. Also cover a failed store initialization after lock acquisition. Use synthetic keys/private temporary profiles, never Keychain or a real profile. Keep existing child-process contention and concurrent-transaction tests.

An unlock error cannot panic in a destructor; closing remains the fallback, with possible conservative contention. Do not claim protection against a process deliberately bypassing cooperative file locking.

## 7. Test-first and review gates

The implementation plan must name a failing test or boundary check before each production behavior/dependency change. Existing characterization assertions are preserved; changing fixture paths/imports is allowed, rewriting expected behavior to accommodate a difference is not.

| Gate | Required evidence |
| --- | --- |
| Lock ownership | Deterministic retained-description RED/GREEN test, last-owner lifetime, failed-open release, child-process exclusion, immediate reopen. |
| Query independence | Query constructed from read/session ports only; search, normalization, and isolated refresh succeed with no cleanup engine or store. A sentinel catalog implementation fails if refresh/preparation calls the global path. |
| Connection | Delayed/failed identity, durable mapping, auth-ready-before-getMe, unchanged progress, no duplicate session or extra credential reads, settings/shutdown races. |
| Recipe parity | Synthetic envelopes and encrypted v3 files produced by the pre-migration codec load under the new codec with identical fingerprint, IDs, steps, counters, and cursor policy. All operation variants and invalid schema/target/effect cases are covered. |
| Cleanup parity | The current recorded native call sequences, batch boundaries, cleanup-before-leave phases, permission changes, retries, cancellation, duplicate start, and persistence failures run against the extracted production lifecycle. |
| Authority boundary | Raw batch calls remain unavailable; no mutation before reviewed authorization, after scope switch, after quarantine, or during an invalid recovery. |
| Dependency boundary | A maintained source/architecture check fails while production query still owns mutation/store access or production composition depends on the old combined service/gateway. Use narrow, documented exceptions for historical readers/test adapters, not directory-wide exclusions. |
| Frontend and generic provider | Existing v2 Rust/TypeScript fixture tests, App tests, stale-response/selection/dirty refresh tests, and nonnumeric synthetic provider continue passing without a Telegram-specific core branch. |

Capture pre-migration encrypted fixtures using deterministic synthetic content-free inputs and test keys only. Do not commit real job stores, chat names, credentials, or exports. If a characterization still requires the old constructor, adapt its harness to the new production path while preserving expectations; do not keep a full second executor solely to make old tests green.

Run focused tests during extraction and full pinned Docker `checks` gates for Linux amd64 and arm64 before handoff. Use existing named BuildKit caches, `--output type=cacheonly`, locked dependencies, and offline/non-root project execution. Do not prune shared caches or perform local host builds. The existing native macOS CI job verifies synthetic tests and the unsigned package on its disposable runner; retain its role because Linux Docker cannot validate the macOS application bundle.

Manual Keychain/LocalAuthentication, VoiceOver, and live Telegram test-DC parity remain separately reported gates. No real Telegram calls or destructive test-DC actions are authorized by this refactor request. Synthetic success is not live parity evidence or release approval.

## 8. Delivery and completion

Implement in reviewable commits: lock prerequisite; pre-migration parity fixtures/boundary tests; native port separation; query/connection extraction; reviewed-cleanup and codec extraction; production cutover and removal of runtime compatibility delegation; full verification/documentation.

The detailed implementation plan will specify exact files, commands, fixture capture, RED/GREEN evidence, and per-slice review checkpoints. Avoid combining a toolchain update, unrelated flaky-test cleanup, or later provider work with this stage. Record any unrelated test failure honestly rather than weakening assertions or serializing tests to obtain a green run.

Completion requires a production dependency graph with genuinely separate read and mutation access, one scoped cleanup-state owner, no `CleanerService`/`TelegramCompatibilityProvider` runtime delegation, unchanged persisted/wire contracts, and passing automated parity gates. Historical schema/type names may remain solely for compatibility. Update the roadmap and characterization documentation to distinguish automated completion from outstanding manual release gates.

## 9. Specification self-review

- [x] Scoped to parent stage 4 plus the approved lock prerequisite.
- [x] Distinguishes module extraction from a neutral executor rewrite or file-only move.
- [x] Defines native port ownership, session guards, sole state writer, and shutdown ordering.
- [x] Preserves wire contracts, recipe schemas, fingerprints, recovery, and explicit effects.
- [x] Names deterministic regression gates and keeps live/manual claims separate.
- [x] User review of this written specification.
- [x] Detailed implementation plan after written-spec approval: [Implementation plan](../plans/2026-09-06-telegram-provider-migration.md).
