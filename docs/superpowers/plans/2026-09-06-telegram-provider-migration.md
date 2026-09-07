# Telegram Provider Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace runtime Telegram compatibility delegation with isolated connection, query and reviewed-cleanup components without changing user-visible or persisted behavior.

**Architecture:** Keep one TDLib session and one scoped cleanup owner. Extract native ports and pure recipe/normalization functions, then separate query ownership and move the existing compound executor into Telegram-owned modules, reusing the shared lifecycle primitives.

**Tech Stack:** Rust 1.97.1, Tauri 2, TDLib 1.8.64, React/TypeScript, existing AES-GCM FoundationStore and SQLCipher archive store; pinned Docker BuildKit checks.

**Spec:** `docs/superpowers/specs/2026-09-06-telegram-provider-migration-design.md`

## Global Constraints

- Keep `RTRCT03`, locator schemas, UUID derivation, metadata schemas, plan canonicalization, and `telegram.compatibility_recipe` version 1 unchanged.
- Preserve the existing action IDs, ordered effect descriptors, confirmation tiers, exact-title checks, prompt sanitization, and one 60-second single-use grant for the same scoped fingerprint.
- Production query objects receive only the session view and read port.
- No query needs a grant, a cleanup-state write, or an archive-store open.
- Scope/generation checks remain before and after awaited provider work, after device-owner prompts, before native mutations, and after retry waits.
- Refresh keeps the existing maximum of 1,000 IDs, deduplication, eight concurrent lookups, ten-second per-chat timeout, title sorting, and omission of missing chats.
- Do not route all compound operations through `FrozenLifecycle` or create a second frozen-batch loop.
- No real Telegram calls or destructive test-DC actions are authorized by this refactor request.
- Use existing named BuildKit caches, `--output type=cacheonly`, locked dependencies, and offline/non-root project execution.
- Do not prune shared caches or perform local host builds.
- Do not push, merge, tag, release, read real profiles/Keychain, or alter dependencies/toolchains as part of task execution.
- All source edits use apply_patch. Mechanical bulk moves/rewrites may use a deterministic script after reviewing its transformations; preserve unchanged function bodies and test expectations.

## File map and command convention

The authoritative existing implementation is the branch base `131005f` (runtime tree equals merged `0d799b0`). Preserve it through fixtures and existing assertions, not a second executor.

New ownership:

- `src-tauri/src/providers/telegram/native/{mod,ports,live_gateway,tdjson}.rs`: native interfaces and existing TDLib implementation.
- `src-tauri/src/providers/telegram/model.rs`: existing native DTOs.
- `src-tauri/src/providers/telegram/{recipe,normalize,diagnostics}.rs`: pure translations and codec.
- `src-tauri/src/providers/telegram/{query,connection,registration}.rs`: provider ports and composition.
- `src-tauri/src/providers/telegram/remediation/{mod,planning,execution,state,authorization,tests}.rs`: extracted reviewed lifecycle.
- `src-tauri/src/providers/telegram/{query_tests,migration_tests}.rs`: new behavioral/parity gates.
- `src-tauri/src/persistence/foundation_lock_tests.rs`: lock regression.
- `scripts/check-provider-boundaries.mjs` and `.node-test.mjs`: architectural lint with controlled-fixture tests.

Focused commands use the existing Dockerfile stage:

```sh
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path src-tauri/Cargo.toml foundation_lock_tests' .
```

Replace only the `RETRACT_CHECK` command for each task. Never mount the host checkout writable or pass credentials into a build. Formatting changes may be obtained from a Docker formatter diff and applied with apply_patch. Run the full Rust suite once before each Rust task commit; full multi-platform/frontend checks belong to Task 6.

## Task 1: Explicit FoundationStore lock ownership

**Files:** Modify `src-tauri/src/persistence/foundation_store.rs`; create `src-tauri/src/persistence/foundation_lock_tests.rs` as a cfg(test) child of foundation_store so it can access the private guard without a production test API.

**Interfaces:** Preserve all FoundationStore public constructors. Private `ProfileLock(File)` replaces `_profile_lock: File`; `acquire_profile_lock(&Path) -> Result<ProfileLock, AppError>` and the private build parameter change together.

- [ ] Add the deterministic retained-description regression before production edits. Build a private temp profile and valid `StoreBinding`, use `open_with_test_key`, retain `store._profile_lock.try_clone()` initially, keep a second Arc, and assert independent acquisition fails until the last Arc drops. After final drop, reopening must succeed even while the retained descriptor lives. Use `open_independent_with_test_key_loader` with a panic key loader for contention, asserting `ProfileInUse`.

```rust
let other_owner = Arc::clone(&store);
let retained = store._profile_lock.try_clone().unwrap();
drop(store);
assert!(matches!(FoundationStore::open_independent_with_test_key_loader(
    profile.clone(), binding.clone(), |_| panic!("lock must precede key")),
    Err(AppError::ProfileInUse)));
drop(other_owner);
let reopened = FoundationStore::open_with_test_key(profile, binding, [7; KEY_LENGTH]);
assert!(reopened.is_ok(), "final store owner must release its lock: {reopened:?}");
drop(retained);
```

- [ ] Run the focused test; require an actual `ProfileInUse` assertion failure at reopen, not a compiler error. Record command/output.
- [ ] Implement the guard; change the test descriptor access to `store._profile_lock.0.try_clone()` without changing its behavioral expectations.

```rust
struct ProfileLock(File);
impl Drop for ProfileLock {
    fn drop(&mut self) { let _ = self.0.unlock(); }
}
// The field remains last; the acquired guard also covers failed key-load/build exits.
// In acquire_profile_lock: Ok(()) => Ok(ProfileLock(file)).
```

- [ ] Add failed-initialization coverage using a retained acquired guard passed to private `build`, an invalid active file, and a subsequent independent lock acquisition. Assert the error leaves active bytes unchanged and releases the lock while the descriptor clone lives. Existing registered-open reuse and child-process tests remain intact.
- [ ] Run focused tests, full backend tests, fmt check and strict Clippy in Docker. Review guard lifetime and failure paths, then commit only this task as `fix: release foundation profile locks explicitly`.

## Task 2: Freeze persisted parity and extract pure codecs

**Files:** Create `telegram/recipe.rs`, `normalize.rs`, `diagnostics.rs`, `migration_tests.rs`, synthetic fixtures under `src-tauri/test-fixtures/telegram-migration/`; modify `telegram/{mod,compat,engine_context,locators}.rs` and dependent tests/imports.

**Interfaces:** Move existing `TelegramExecutionRecipe`, `TelegramFrozenItem`, and `EXECUTION_SCHEMA` unchanged to recipe. Produce free functions `bind_plan(scope: &Scope, plan: &mut cleaner_domain::DeletionPlan) -> Result<RemediationPlan, AppError>` and `normalize_job(scope: &Scope, plan: &DeletionPlan, job: &JobRecord, started_authorized: bool) -> Result<ScopedJobRecord, AppError>`, copying the existing associated-function signatures exactly if the inspected name differs. Move `normalize_conversation`, `normalize_content`, metadata structs, descriptor construction, and safe diagnostic conversion without serialized changes. Codec validation calls free `bind_plan`, not the provider instance.

- [ ] Before extraction, capture fixed-UUID/fixed-time synthetic envelopes and actual authenticated v3 store bytes using the base codec for all nine native PlanOperation variants; include selected targets `9007199254740992` and `9007199254740993`, negative chat IDs, mixed reaches, compound phases, authorized/unauthorized jobs, retry metadata, and foreign scope. Use test keys only. Retain a readable manifest of base commit and expected fingerprints.
- [ ] Add characterization tests loading the frozen bytes, comparing exact expected envelopes/fingerprints and recovery decisions. Test the existing implementation first; mutate one frozen effect/locator/recipe version and require rejection. Expectations come from immutable captured artifacts, not the codec under test.

```rust
let expected: RemediationPlan = serde_json::from_str(include_str!("../../../test-fixtures/telegram-migration/selected.json")).unwrap();
let native = TelegramExecutionRecipe::validate_envelope(&expected).unwrap();
let mut changed = expected.clone();
changed.recipe.version = 2;
assert!(TelegramExecutionRecipe::validate_envelope(&changed).is_err());
assert_eq!(native.fingerprint, expected.fingerprint);
```

- [ ] Extract pure functions and move their callers. Do not reimplement plan-binding logic, rename schema strings, alter validator policy keys, or move state ownership yet. A temporary facade may forward to free functions while Task 5 removes it.
- [ ] Run migration tests, provider engine tests, persistence tests, and the complete backend suite. Compare frozen artifact bytes before/after; record no format migration. Run fmt/Clippy and commit `refactor: isolate Telegram recipes and normalization`.

## Task 3: Separate native ports and contain native implementation

**Files:** Create `telegram/native/{mod,ports}.rs`; move root `live_gateway.rs`, `tdjson.rs` into native and root `model.rs` into telegram; modify root `lib.rs`, `gateway.rs`, old service, demo/setup gateways, `telegram/engine_context.rs`, and import consumers. Create `scripts/check-provider-boundaries.mjs` and `scripts/check-provider-boundaries.node-test.mjs`; wire architectural check into Docker `checks` and focused command capability.

**Interfaces:** `TelegramSession: Send + Sync` exposes current `info`, `auth`, `verified_identity`, `catalog_progress`; `TelegramRead: TelegramSession` exposes existing read method signatures; `TelegramMutation: TelegramSession` exposes only existing mutation signatures; `TelegramConnectionIo: TelegramSession` exposes existing typed auth/close signatures. Use async_trait on async ports. Keep native argument/result types unchanged and explicitly qualified under Telegram.

```rust
#[async_trait::async_trait]
pub trait TelegramRead: TelegramSession {
    async fn chats(&self) -> Result<Vec<ChatSummary>, AppError>;
    async fn chat_by_id(&self, chat_id: i64) -> Result<Option<ChatSummary>, AppError>;
    async fn search(&self, request: &SearchRequest) -> Result<Vec<MessageSnapshot>, AppError>;
    // Move own_messages, chat_messages, messages_by_ids, sender_name, current_reach
    // with their exact signatures from the current gateway.rs declaration.
}
```

- [ ] Add an architectural lint that checks imports and prohibited production dependencies by module ownership, with explicit file-level transitional exceptions for old service/facade removed by later tasks. Its test runner creates controlled valid and invalid Rust module trees and asserts success/failure, so the lint's rejection logic is executed, not tested by grepping its source. First require native implementation/model to be in the provider boundary: running against current tree must fail for actual misplaced dependencies.
- [ ] Move native files and split existing LiveGateway impl blocks between the four traits, preserving method bodies. Update DemoGateway/SetupGateway implementations as test adapters. A temporary combined supertrait may support the old executor but production query in Task 4 must not use it; remove it in Task 5.
- [ ] Split the session guard's forwarding by these same ports, preserving before/after checks and post-send ambiguity behavior. Use trait-object upcasting or separate Arcs to the same instance, not another session. Retain `SessionBinding`/`EngineContext` identity validation and never convert identity to a display-name check.
- [ ] Move native models and update imports in historical readers and UI setup composition. Keep historical v1 fixture shapes and serialized fields unchanged. Move native error interpretation into Telegram diagnostics while shared storage errors stay in core; exact v2 code mapping remains covered.
- [ ] Run native mapping/search/auth tests, existing engine scope/ambiguity tests, lint controlled-input tests, full backend suite and fmt/Clippy. Commit `refactor: separate Telegram native ports`.

## Task 4: Independent query and connection components

**Files:** Create `telegram/{query,query_tests,connection,registration}.rs`; modify `telegram/{mod,application,compat}.rs`, native port consumers and root `lib.rs` composition.

**Interfaces:** `TelegramQuery::new(read: Arc<dyn TelegramRead>, context: Arc<EngineContext>) -> Result<Self, AppError>` implements both `QuerySource` and `ApplicationQuery`. It owns no cleanup/store/mutation object. `TelegramProvider` implements `ProviderRegistration`, returning Arc clones of separate query and reviewed-lifecycle objects. `TelegramConnection` retains existing constructor and implements `ApplicationConnection` using one LiveGateway and FoundationStore; constructs the new registration after verified identity.

- [ ] Add RED behavioral ownership test against existing registration: retain only returned query port, drop the provider and original cleanup owner, and assert a Weak cleanup owner cannot upgrade; current facade clones retain the engine and fail. The query must still return normalized synthetic results after cleanup drops.

```rust
let weak_cleanup = Arc::downgrade(&engine);
let query = registration.application_query().unwrap();
drop(registration);
drop(engine);
assert!(weak_cleanup.upgrade().is_none(), "query must not retain cleanup state");
let page = query.search_filtered(&context, request).await.unwrap();
assert_eq!(page.items.len(), 1);
```

- [ ] Extract both query impls, native request validation, conservative truncation, and isolated refresh. Use only read/session ports, and move direct lookup helper with existing 10-second bound/8-way concurrency. Normalization uses Task 2 functions. Keep scope checks on both sides of awaits. Do not change cursor support or schemas.
- [ ] Split connection/bootstrap and registration from `application.rs`; registration returns independent query/cleanup handles. For this intermediate task only, the cleanup port may still be the old facade; no query retains it. No auth path opens a second TDLib runtime or reads a new secret.
- [ ] Add/retain behavioral tests for duplicate/zero/oversized refresh, sorted/missing chats, zero catalog reads during isolated refresh/preparation, query/resolve stale context, unsupported cursor, full-page truncation, auth identity races and progress. A read-only fake implements no mutation/auth-write trait and constructs the production query successfully.
- [ ] Run focused query/connection/provider tests, complete backend suite, frontend v2 contract tests, fmt/Clippy; commit `refactor: decouple Telegram queries and connection`.

## Task 5: Extract reviewed cleanup and remove runtime compatibility delegation

**Files:** Create `telegram/remediation/{mod,planning,execution,state,authorization,tests}.rs`; modify `telegram/{registration,connection,engine_context,mod,recipe,diagnostics}.rs`; remove production root `service.rs`, combined `gateway.rs`, `telegram/compat.rs` and `application.rs` after moving remaining logic/tests; update test/import consumers and boundary-lint exceptions.

**Interfaces:** `TelegramCleanup::new_scoped(read: Arc<dyn TelegramRead>, mutation: Arc<dyn TelegramMutation>, context: Arc<EngineContext>, store: Arc<dyn TelegramStateRepository>) -> Result<Arc<Self>, AppError>` replaces `CleanerService::new_scoped`. `TelegramCleanup` implements `ReviewedLifecycle` directly. State fields are owned once by cleanup; planning/execution/authorization are focused impl modules, not separately mutable services. Existing `EngineContext`, `FoundationTelegramRepository`, pure recipe codec, GrantBook, ScopedRepository and run_frozen_batches remain the shared contracts.

- [ ] Tighten the already-tested dependency lint to reject old combined service/gateway/facade use by production. Record the intended RED result before cutover. Retain all Task 2 ciphertext/fingerprint tests and Task 4 query isolation tests.
- [ ] Extract the service's plan preparation, transition, grant, authorization, job scheduling, worker stop, recovery and compound execution functions into focused cleanup modules. Preserve bodies/ordering; replace only gateway reads with `read` and native writes with `mutation`, both session-guarded. Keep private fields narrowly visible to remediation child modules.
- [ ] Move the reviewed-lifecycle adapter methods directly onto cleanup, returning normalized plans/jobs from pure codec functions. Move action discovery to planning. Registration returns the same cleanup Arc for every lifecycle request; no wrapper embeds another full cleanup service. Queries stay on their independent handle.
- [ ] Adapt historical test harness constructors to build this same cleanup implementation using synthetic adapters; keep the old legacy repository test-only. Do not preserve a duplicate executable legacy implementation. Remove obsolete snapshot/auth/search methods from cleanup once their consumers moved; historical DTO fixtures retain shape tests without requiring those runtime methods.
- [ ] Run existing operation call-sequence tests for selected/own/clear/self/leave/admin/sender/destroy, grant expiry/reuse/context switch, post-send ambiguity, retry recheck, duplicate start, failed writes, cursor recovery, foreign scopes and settings shutdown. Add a retained-mutation-sentinel test that calls public raw batch APIs and proves no native mutation occurs without reviewed authorization.

```rust
// Reuse the existing observed native boundary: request acceptance is not a grant.
assert!(registration.remediation().is_err());
assert_eq!(observed.native_mutation_count(), 0);
// Actual ReviewedLifecycle::start without authorize must fail and preserve count.
assert!(cleanup.start(&context, unapproved_start).await.is_err());
assert_eq!(observed.native_mutation_count(), 0);
```

- [ ] Run all backend tests, boundary lint, frontend tests, fmt/Clippy, compare frozen fixtures and inspect the production dependency graph. Commit `refactor: migrate Telegram reviewed cleanup into provider`.

## Task 6: Whole-branch acceptance and documentation

**Files:** Update `PLAN.md`, `docs/TELEGRAM_CHARACTERIZATION.md`, `docs/TEST_PLAN.md`, this plan's progress; only amend implementation files to address reviewed failures through the fix workflow.

**Interfaces:** No new runtime interfaces; consume the fully migrated query/connection/cleanup components and Task 2 artifact manifest.

- [x] Run the final architecture lint and controlled-input tests, all frozen codec/ciphertext comparisons, existing Rust/TypeScript contracts, and full container gates on both architectures:

```sh
docker buildx build --platform linux/arm64 --target checks --output type=cacheonly --progress plain .
docker buildx build --platform linux/amd64 --target checks --output type=cacheonly --progress plain .
```

- [x] Record exact head, commands, counts, skipped/manual gates and any initial failures. A cached gate only proves its input tree; record when a gate is reused. Do not run the archive corpus benchmark for this non-indexing refactor.
- [x] Update docs to reflect direct Telegram provider ownership and explicit foundation lock release, retaining historical evidence and manual live/native limits. Do not mark manual test-DC parity complete.
- [x] Commit documentation, obtain whole-branch review of all task commits, and run covering tests for reviewed fixes. Leave the branch ready for the user's push/PR or merge decision; native macOS CI runs when a branch/PR is authorized and published, unless already accessible for the exact final head. Report it pending rather than claiming a local Linux build validates macOS.

## Progress

- [x] Task 1: Foundation lock ownership.
- [x] Task 2: Persisted parity and pure codecs.
- [x] Task 3: Native port boundaries.
- [x] Task 4: Independent query and connection.
- [x] Task 5: Reviewed cleanup cutover.
- [x] Task 6: Full acceptance and documentation — implementation, full Linux acceptance, documentation and independent whole-branch review are complete; its documentation-only finding closure is recorded separately and awaits scoped re-review.

Reviewed and tested acceptance source: commit `c7f2f5f29387a71c9ac4051b7dd66508baee1311`. Full Linux `arm64` and `amd64` checks copied that exact committed tree and executed the project checks, passing 329 backend tests (one existing opt-in archive corpus test ignored), 158 frontend tests, 17 cleaner-domain tests, 19 retract-domain tests, 7 release tests, 10 controlled provider-boundary tests plus the real-tree lint, production-bundle/public-repository checks, formatting and strict Clippy. BuildKit results were `yafbi07vc0a8p6gv3hjryumwt` (`arm64`) and `ado6le3q5vvow7wk1pojcyq9u` (`amd64`). Independent whole-branch review covered `0d799b0..c7f2f5f`, found no Critical or Important issues, and requested this documentation-only closure for two Minor findings. The earlier clean runtime commit `3a78bd207d89be3d3bc93b8cfac6930ce04017e6` remains historical acceptance evidence. The archive corpus benchmark was not run. Native macOS CI, real Keychain/LocalAuthentication, VoiceOver/rendered viewport checks and disposable Telegram test-DC parity are pending and are not implied by the Linux result. The later documentation-only closure receives a focused public-repository check and controller-owned scoped re-review; no future check or review is claimed here.
