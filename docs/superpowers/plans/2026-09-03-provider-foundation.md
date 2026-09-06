# Provider Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the real Telegram application through scoped opaque identities, a versioned compatibility API, and authenticated account-bound plans/jobs without rewriting TDLib behavior.

**Architecture:** A neutral Rust domain and explicit provider registry sit above a Telegram compatibility adapter. That adapter delegates to the existing engine through a scoped context and transactional repository, preserving its mutation ordering while replacing unscoped IPC and persistence. The frontend switches atomically to version 2 after the backend path exists.

**Tech Stack:** Rust 2024, Tauri 2, serde, UUID v4/v5, SHA-256, AES-GCM, React 19, TypeScript 6, Vitest 4, Docker BuildKit; retain repository-pinned toolchains and lockfiles.

**Spec:** `docs/superpowers/specs/2026-09-03-provider-foundation-design.md`

**Completion tracking (2026-09-04):** Tasks 1–9 are implemented and independently reviewed. Whole-branch review findings were addressed in `6207734`; scoped re-review found no remaining issues. Checked historical RED steps record evidence captured before implementation, not intentionally failing tests left in the current suite. Native macOS packaging/live Telegram and native/browser modal geometry need separate evidence and are not implied by Docker or jsdom results. The branch is ready for the user's integration decision; nothing has been merged or published.

## Global Constraints

- The bridge is transitional; it does not replace TDLib, reimplement Telegram policy, or relocate the entire legacy service.
- No SQLCipher, content indexing, archive ingestion, Discord/X integration, token handling, or provider UI redesign.
- Keep TDLib files, connection-settings schema, vault format, and existing keychain entries unchanged.
- Provider/account/source scope, opaque identifiers, and the session generation must be checked on the real application path.
- No production identifier changes before Task 1 records behavior-level RED evidence for selection, IPC, planning, jobs, encrypted recovery, and targeted reconciliation.
- Everyone-scoped deletion must never fall back to self-only deletion or another effect.
- Preserve exact review/confirmation, the 60-second single-use owner grant, bounded calls, per-message checks, cancellation, and cleanup-before-leave ordering.
- All provider-native interpretation is inside the owning provider/legacy boundary; no frontend numeric conversion of identifiers.
- Persist no message bodies, sensitive attachment names, credentials, auth values, tokens, or raw provider errors in jobs/diagnostics.
- One version-3 profile store writer; serialize state mutation and persistence together; no mutation may proceed after a required save fails.
- Preserve old encrypted bytes before migration, stop all unbound nonterminal legacy jobs with `migration_requires_new_review`, and never automatically fall back to the backup.
- Use synthetic fixtures only. No real Telegram session, deletion, keychain mutation, or private archive is needed for implementation verification.
- Build/test inside Docker with locked dependencies, non-root/offline project execution, shared named caches, and `--output type=cacheonly`. Do not prune global Docker state.
- Use `apply_patch` for source/document edits. Generated lockfiles/build artifacts may be exported by tools. Preserve unrelated work.

## Execution notes

The user approved the specification and requested implementation, so execute all tasks without another design/plan approval loop. Do not merge, push a shared branch, tag, or release as part of this request. Record task commits and RED/GREEN evidence in this plan's local SDD ledger.

Task 1 intentionally introduces failing regression tests. Do not skip them or invert their assertions to make intermediate commits green. Later tasks may move the tested call boundary from the retired v1 API to its actual v2 replacement; the asserted identity/scope/recovery behavior must stay unchanged. The full gate cannot pass until the cutover is complete. Existing characterization tests continue to prove the legacy reader and engine contracts.

### Contract decisions shared by the tasks

- All new Rust DTO fields use `camelCase`; enum values use `snake_case`. Wire version is the literal integer `2`; encrypted store schema/header are `3`/`RTRCT03`.
- `Scope` has `provider: ProviderKey`, `account_id: AccountId`, `source_id: SourceId`. `ActiveContext` adds `session_generation: Uuid`.
- `ProviderResourceRef` has the fields specified in the approved design. `ScopedResourceRef` adds `scope: Scope` and `id: Uuid` beside `resource: ProviderResourceRef`. The embedded provider/account must equal scope. Source and application resource ID must be validated, not merely carried.
- `VersionedPayload` is `{ schema: String, version: u16, payload: serde_json::Value }`. Only owning provider code interprets its payload.
- `ResourceKind` includes `conversation`, `content`, `actor`, and `grouping`. Derived UUID v5 names serialize `["retract-resource-v1", resourceKind, locatorSchema, locatorVersion, canonicalKey]` as compact JSON; namespace is the account UUID. Reject conflicting full references sharing an ID within results/state/plans.
- Telegram locator schemas are `telegram.account`, `telegram.conversation`, `telegram.message`, `telegram.actor`, `telegram.grouping`, version `1`. Typed payloads reject unknown fields. Native IDs use canonical decimal strings: no `+`, whitespace, redundant leading zeros, or float/exponent notation. Chat IDs are nonzero signed `i64`; message/user IDs are positive `i64`; actor payloads distinguish `user` and `chat`; grouping includes its chat ID.
- Telegram account identity payload is `{environment: "production" | "test", userId: string}`. It is obtained from authenticated backend state, never frontend requests.
- `ActionDescriptor` uses the nine kinds, seven effects, and four availability states in the parent design. An ordered plan step has one descriptor and explicit target references. Compound cleanup/leave plans retain multiple ordered effects.
- `RemediationPlan` is backend-owned: UUID `id`, `scope`, ordered `steps`, canonical `targets`, `confirmation`, versioned `recipe`, `restart_policy`, `created_at`, and `fingerprint`. `ConfirmationRequirements` carries the existing tier and optional exact title/name. Restart policy is `frozen_idempotent` or `new_review_required`.
- `ScopedJobRecord` contains UUID `id`/`plan_id`, scope, frozen dirty targets, a status including the six current values plus `blocked`, existing counters/cursor/retry/timestamps, safe diagnostic codes, and durable `started_authorized: bool`. `LegacyJobHistory` is explicitly unbound and has no executable retry path.
- Version-2 command envelopes are `CommandRequest<T> {contract_version: u16, context: ActiveContext, payload: T}` and `CommandResponse<T> {contract_version: u16, context: ActiveContext, payload: T}`. Bootstrap uses a separate response with optional active context and an identity-preparation stage; do not invent scope before account verification.
- New domain structs are independent of `cleaner-domain`. Existing native structs remain legacy adapter implementation details, not aliases for neutral records.

## File organization

`crates/retract-domain/src/{identity,records,actions,plans,error}.rs` owns neutral types and invariants. `src-tauri/src/providers/{ports,registry}.rs` owns dispatch. `providers/telegram/{identity,locators,compat,engine_context}.rs` owns the Telegram bridge. `persistence/{foundation_store,migration,model}.rs` owns version-3 state. `compatibility/{model_v2,commands_v2}.rs` and `provider_service.rs` own the new application boundary. Existing service/gateway changes are limited to explicit bridge seams.

Frontend runtime validators and keys live in `src/providers/{contract,identity}.ts`; production and screenshot APIs consume them. Preserve `src/types.ts` as the application's exports and put historical v1 DTOs in `src/test/legacy-types.ts` for the frozen contract test rather than weakening that historical fixture.

---

### Task 1: Establish the RED opaque-identity lifecycle gate and focused container runner

**Files:**
- Create: `src/test/fixtures/provider-lifecycle.json`, `src/provider-lifecycle.test.tsx`, `src-tauri/src/foundation_lifecycle_tests.rs`.
- Modify: `src-tauri/src/lib.rs` (test module only), `Dockerfile`, `.dockerignore`.
- Read/Test: `src/components/ResultsList.tsx`, `src/App.test.tsx`, `src/api.desktop.ts`, `src-tauri/src/service.rs`, `src-tauri/src/secure_store.rs`.

**Interfaces:**
- Consumes: current `messageKey`, desktop adapter, `CleanerService`, `SecureJobStore`, and synthetic gateway helpers.
- Produces: a shared synthetic fixture and named RED tests, without production ID changes; `focused-checks` Docker target accepting `RETRACT_CHECK`.

- [x] Record the successful baseline `npm run container:check` result before changes. Refactor the Docker check environment into a shared check base and add a sibling focused target using the same non-root user, offline mode, and named Cargo caches:

```dockerfile
FROM check-base AS focused-checks
ARG RETRACT_CHECK="npm test"
RUN --network=none \
    --mount=type=cache,id=retract-cargo-home,target=/home/retract/.cargo,uid=10001,gid=10001,sharing=locked \
    --mount=type=cache,id=retract-cargo-target-${TARGETARCH},target=/home/retract/.cache/retract-target,uid=10001,gid=10001,sharing=locked \
    sh -eu -c "$RETRACT_CHECK"
```

Keep the existing `checks` and `frontend-artifact` targets intact. Add `.worktrees` and `.superpowers` to `.dockerignore` so scratch reports and other checkouts neither enter builds nor invalidate caches. Run existing focused frontend and Rust tests through this target before adding expected failures.

- [x] Write complete synthetic fixture records using account UUIDs `aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa` and `bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb`, source UUIDs `11111111-1111-4111-8111-111111111111` and `22222222-2222-4222-8222-222222222222`, providers `telegram` and `synthetic`, and native strings `9007199254740992`, `9007199254740993`, `message:part/0007`, and `-1001`. Include full legacy-compatible display fields and v2 scope/ref data; no real account data.

- [x] Add real key/selection and desktop IPC boundary tests. Begin with this current-boundary regression (fixture objects retain extra scope fields at runtime):

```ts
const first = fixture.messages[0];
const otherAccount = { ...first, scope: fixture.otherScope };
expect(messageKey(first as unknown as MessageSnapshot))
  .not.toBe(messageKey(otherAccount as unknown as MessageSnapshot));
```

Also test two distinct large-ID targets surviving selection/preparation, a stale old-context refresh not clearing the current selection, and desktop dispatch using v2 context rather than naked numeric refs. Render actual components and mock only IPC/network boundaries. Derive expected target strings literally, not by the tested key helper.

- [x] Add backend RED assertions through the existing request/service/store paths: new string refs must not lose precision or scope, account changes must change the plan binding, jobs must retain scoped dirty refs after an actual encrypted save/reload, and a request for another account must not mutate the synthetic gateway. Initially drive the legacy parser/engine to expose its missing v2 behavior; do not implement a test-only replacement engine. For example, the existing `MessageRef` parser rejects the required string wire shape:

```rust
let parsed = serde_json::from_value::<crate::model::MessageRef>(
    serde_json::json!({"chatId":"-1001","messageId":"9007199254740993"})
);
assert!(parsed.is_ok(), "the application boundary must accept lossless string identifiers");
```

This is a migration RED assertion, not a demand to change the frozen legacy DTO; Task 6 rewires it to the real version-2 parser. Add distinct plan/job/recovery/refresh assertions so this parser test is not the entire lifecycle gate.

- [x] Run and record the expected failures separately so a frontend failure cannot hide the Rust results:

```bash
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=npm test -- src/provider-lifecycle.test.tsx' --output type=cacheonly --progress plain .
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path src-tauri/Cargo.toml foundation_lifecycle' --output type=cacheonly --progress plain .
```

Failures must name missing identity/scope behavior, not missing modules, syntax errors, or broken fixtures. Commit the regression gate with its RED evidence. No production type migration is allowed until all lifecycle categories have evidence.

---

### Task 2: Add the neutral domain, canonical identities, actions, and frozen plan envelopes

**Files:**
- Create: `crates/retract-domain/Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/identity.rs`, `src/records.rs`, `src/actions.rs`, `src/plans.rs`, `src/error.rs`, `tests/contracts.rs`.
- Modify: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `Dockerfile`, `package.json`, `.github/dependabot.yml`, `scripts/check-public-repo.mjs`, `scripts/release-metadata.mjs`, `scripts/release-metadata.node-test.mjs`.

**Interfaces:**
- Consumes: the shared contract decisions above, Task 1 fixture and recorded RED evidence.
- Produces: `Scope`, `ActiveContext`, `ProviderResourceRef`, `ScopedResourceRef`, normalized records, descriptors/errors, `RemediationPlan`, `ScopedJobRecord`; `ProviderResourceRef::resource_id() -> Result<Uuid, DomainError>`, `RemediationPlan::seal() -> Result<(), DomainError>`, `RemediationPlan::validate() -> Result<(), DomainError>`.

- [x] Write failing contract tests for provider-key validation, UUID string serialization, scope equality, provider/account/source mismatch, canonical UUID stability, and conflicting-reference detection. Use literal fixture values. Assert distinct resources for the two large native strings, not numeric casts.

- [x] Implement typed validated IDs and refs. Provider keys use lowercase ASCII `[a-z][a-z0-9_-]{0,63}`. Validate nonempty bounded schema/canonical fields, exact scope agreement, and derived resource IDs. Use standard UUID v5 and the shared canonical tuple; use existing serde/chrono/uuid/sha2/thiserror versions, enabling uuid's v5 feature. Do not introduce network/filesystem dependencies.

```rust
pub fn resource_id(&self) -> Result<Uuid, DomainError> {
    let name = serde_json::to_vec(&(
        "retract-resource-v1", self.resource_kind,
        &self.locator_schema, self.locator_version, &self.canonical_key,
    )).map_err(|_| DomainError::InvalidReference)?;
    Ok(Uuid::new_v5(self.account_id.as_uuid(), &name))
}
```

- [x] Define full normalized records from parent-design sections “Account and source”, “Conversation”, “Content item”, and “Attachments”: IDs, provenance, evidence/timestamps, inert attachments, optional parent/reply, privacy findings and versioned metadata. Neutral records retain an opaque versioned metadata envelope; Tasks 4–5 define the outgoing/pin/grouping/filter metadata type inside the Telegram adapter, not in this neutral crate. Reuse existing privacy-kind semantics; do not duplicate the detector implementation.

- [x] Add tests that alter each fingerprint-bound field, reorder object insertion, reorder unordered targets, and reorder ordered action steps. Implement canonical JSON encoding, target sorting/deduplication with conflicting-identity rejection, scope validation, confirmation requirements, and restart policy. The recipe excludes its own fingerprint to avoid circular hashing. A sealed plan validates by recomputing its fingerprint.

```rust
let original = fixture_plan();
let mut changed = original.clone();
changed.scope.account_id = other_account();
assert!(changed.validate().is_err());
let mut reversed_steps = original.clone();
reversed_steps.steps.reverse();
assert!(reversed_steps.validate().is_err());
```

- [x] Add all action/effect/availability and provider/application error variants specified by the design. Map safe messages from codes; no arbitrary error details bag. Define blocked jobs and explicit legacy-history types, not guessed account scope.

- [x] Generate/update Cargo locks inside the pinned container, preserving unrelated locked versions. Export only generated lockfiles. Add the new crate to strict Docker fetch/test/fmt/Clippy, npm full checks, Dependabot, MIT/version consistency, and release version-update tests. Test release metadata against a temporary fixture repository rather than source-text assertions.

- [x] Run focused domain contracts and release metadata tests, format/Clippy all new targets, and unchanged cleaner-domain tests. Commit the domain and its build integration. Task 1 end-to-end tests may remain RED until their production consumers are migrated.

---

### Task 3: Build transactional version-3 persistence and conservative legacy migration

**Files:**
- Create: `src-tauri/src/persistence/mod.rs`, `model.rs`, `foundation_store.rs`, `migration.rs`, `tests.rs`.
- Modify: `src-tauri/src/secure_store.rs`, `src-tauri/src/lib.rs` (module declarations), `src-tauri/src/error.rs`.

**Interfaces:**
- Consumes: neutral identity/plan/job types and the current authenticated legacy `PersistedState` reader.
- Produces: `FoundationState` with identities, sources, plans, jobs, legacy history and migration provenance; `StoreBinding { provider: ProviderKey, profile: String }`; `FoundationStore::open(profile: PathBuf, binding: StoreBinding) -> Result<Arc<Self>, AppError>`, `snapshot() -> Result<FoundationState, AppError>`, `transaction<T>(&self, change: impl FnOnce(&mut FoundationState) -> Result<T, AppError>) -> Result<T, AppError>`. The provider adapter/runtime supplies the expected binding from backend configuration, never frontend display data. Include it explicitly in authenticated associated data and validate row ownership; do not infer the provider from directory-name conventions.
- Test constructors inject a key/profile and private temporary path without touching the real vault. Production uses the existing job-store key through the existing cache.

- [x] Write RED migration tests using the frozen `RTRCT01`/`RTRCT02` ciphertext fixtures and real temporary files. Cover exact backup-byte preservation, terminal history, every old nonterminal status becoming failed/partial with `migration_requires_new_review`, no inferred account, unknown/corrupt headers, wrong profile, and missing active file with existing backup/artifacts.

```rust
let before = std::fs::read(&legacy_path).unwrap();
let store = open_fixture_store(&legacy_path).unwrap();
assert_eq!(std::fs::read(legacy_path.with_file_name("jobs.pre-provider.enc")).unwrap(), before);
assert!(store.snapshot().unwrap().jobs.is_empty());
assert!(store.snapshot().unwrap().legacy_history.iter().all(|row| !row.executable));
```

`open_fixture_store` is a test utility wrapping the production store constructor with a synthetic key, not an alternate migration implementation.

- [x] Extract reusable authenticated read/write/key access from `secure_store.rs` without changing the legacy decoder/vault contract. Implement `RTRCT03`, schema/profile/provider AAD, private file permissions, and an interprocess exclusive profile lock. A second independent handle/process must report `profile_in_use`; within-process callers share the same `Arc`.

- [x] Implement migration exactly in spec section 7: exclusive byte-for-byte backup creation and sync; transform legacy rows without assigning scope; private new file; decrypt/validate the actual candidate; atomic replace/sync; keep backup; no automatic fallback. Fresh profiles initialize v3 only if no migration evidence exists. Setup-only mode never creates verified live state with the setup key.

- [x] Write RED transactional tests for failed private write, failed verification, and concurrent stale snapshots. Implement a single serialized transaction that clones the last committed state, applies/validates the change, durably saves, then publishes it. If replacement succeeded but final durability confirmation fails, quarantine the writer and reload/validate before another transaction; never keep writing from a known stale in-memory snapshot.

```rust
let candidate = current.clone();
// Apply the caller's mutation to a mutable candidate, validate its relationships,
// write and authenticate it, and only then replace the committed in-memory state.
// The lock covers the full sequence, including the durable write.
```

Use real failure-producing paths/permissions or narrowly injected filesystem operations in test utilities. Production objects do not expose test-only cleanup methods.

- [x] Validate plans/fingerprints, unique identities, plan/job references, source/account relationships, cursor bounds, and content-free provider recipes on load/commit. Unknown provider recipe versions remain non-executable until validated by the adapter.

- [x] Run the persistence suite, all existing secure-store tests, and Clippy in Docker. Confirm old binaries' reader rejects the current v3 header while the backup still decodes historically. Commit the store/migration.

---

### Task 4: Add focused provider ports, the registry, and verified Telegram identity/locators

**Files:**
- Create: `src-tauri/src/providers/mod.rs`, `ports.rs`, `registry.rs`, `telegram/mod.rs`, `telegram/identity.rs`, `telegram/locators.rs`, `tests.rs`.
- Modify: `src-tauri/src/gateway.rs`, `src-tauri/src/live_gateway.rs`, `src-tauri/src/setup_gateway.rs`, `src-tauri/src/demo_gateway.rs`, `src-tauri/src/lib.rs`.

**Interfaces:**
- Consumes: neutral domain and `FoundationStore::transaction`.
- Produces: `ProviderRegistry::register(Arc<dyn ProviderRegistration>) -> Result<(), ProviderError>`, `get(&ProviderKey) -> Result<Arc<dyn ProviderRegistration>, ProviderError>`; focused ports matching the parent design; `SessionBinding` that validates/invalidates `ActiveContext`; typed Telegram locator encode/decode/normalize helpers.
- Add `TelegramGateway::verified_identity() -> Option<VerifiedTelegramIdentity>`; the type contains backend-verified environment/user ID and session generation. Setup returns `None`; synthetic gateways return only explicit test identities.

- [x] Add RED registry tests for duplicate keys, unknown providers, missing ports, and a nonnumeric synthetic query provider. Implement explicit dispatch, descriptor capability data, query pagination/resolve contracts, remediation preflight/batch/verification contracts, typed connection state, and import inspection/sink contracts without a production archive implementation. Define their request/result structs in `ports.rs`; do not use success-returning stubs for unsupported ports.

- [x] Add RED locator tests for large/signed IDs, user-vs-chat actors, same message ID in different chats, wrong canonical keys, unknown schemas/fields, out-of-range values, and noncanonical decimal forms. Implement serde typed payloads with `deny_unknown_fields`, canonical tuple keys, and account-namespace UUID derivation. Native parsing is confined to this module.

- [x] Add RED production TDJSON tests for ready-before-`getMe`, failed/malformed `getMe`, sign-out, reconnect, and an account changing during an asynchronous request. Implement identity only after valid authenticated `getMe`; clear it on nonready transitions; use a new generation per authenticated session. Do not change the existing auth input flow or initiate global catalog refresh for identity lookup.

```rust
assert!(gateway.verified_identity().is_none());
// Deliver authorizationStateReady, then hold the scripted getMe response.
assert!(gateway.verified_identity().is_none());
// Resolve getMe with a valid synthetic user, then verify its identity is visible.
```

- [x] Persist verified identity/source mappings transactionally before returning active context. Reuse IDs across rename/restart; separate production/test environments and different accounts. Reject failed persistence without publishing a transient context. Invalidated contexts cannot be reactivated through display data.

- [x] Run provider/locator/TDJSON identity tests plus the existing live-gateway characterization suite. Commit registry and identity foundations.

---

### Task 5: Bind the existing Telegram engine to scoped plans, one grant, and the v3 repository

**Files:**
- Create: `src-tauri/src/providers/telegram/engine_context.rs`, `compat.rs`, `engine_tests.rs`.
- Modify: `src-tauri/src/service.rs`, `src-tauri/src/gateway.rs`, `src-tauri/src/model.rs`, `src-tauri/src/persistence/model.rs`, `src-tauri/src/persistence/foundation_store.rs`.

**Interfaces:**
- Consumes: verified session binding, canonical locator helpers, `RemediationPlan`, registry ports, and transactional store.
- Produces: a `TelegramCompatibilityProvider` implementing query/remediation wrappers and a scoped legacy-engine context. `CleanerService::new_scoped(gateway: Arc<dyn TelegramGateway>, context: Arc<EngineContext>, repository: Arc<dyn TelegramStateRepository>) -> Result<Arc<Self>, AppError>` constructs a service for one verified account/source; production no longer uses an unscoped constructor. `EngineContext` owns the immutable verified context, the revocable session binding, and the plan-binding hook; the repository never invents account identity.
- Introduce an internal `TelegramStateRepository` port with `load() -> Result<PersistedState, AppError>` and `save(&PersistedState) -> Result<(), AppError>`. The production implementation projects/updates only its validated scope within v3; the legacy store implementation exists only for historical test coverage.

- [x] Write RED tests for same targets under different accounts/sources producing different fingerprints, recipe-vs-target disagreement, stale identity before mutation, and a changed context while the owner-authentication prompt is pending. Use actual plan builders/executor and mock only the native authentication prompt and gateway I/O.

- [x] Normalize query results and translate all current operations into backend action descriptors and ordered effects. Keep the existing Telegram capability/policy implementation as source of truth. Broad operations retain explicit broad semantics; permanent destruction remains independent. Store a typed version-1 Telegram execution recipe excluding its fingerprint field.

- [x] Add one plan-binding hook before persistence/review. It creates/seals the neutral envelope, then assigns its fingerprint to the legacy plan before any `PlanView` or grant exists. The v3 repository verifies envelope/recipe/legacy-plan agreement when loading and saving. Never authorize an inner native fingerprint separately.

- [x] Extend grants with scope/session generation and recheck after the native prompt and before consumption. Use a guarded gateway/session handle to check identity before every provider call and after waits. A live handle cannot silently become a different account. Preserve existing batch/preflight/cleanup ordering and duplicate-start rejection.

- [x] Replace ignored recovery persistence results with fail-closed outcomes. Ensure failed saves do not leave executable plans/jobs published in memory, and prevent out-of-order per-scope snapshots overwriting advanced progress. Serialize state transitions through the repository; introduce focused helpers instead of copying the execution loop.

- [x] Write and pass recovery tests: legacy rows never enter the scoped executor; matching authorized frozen v3 jobs resume at a valid cursor; other accounts become `blocked`; blocked jobs permit changing settings; broad/ambiguous/unknown-schema work requires review; failed progress save prevents the next call. Keep all historical legacy service tests runnable against their explicit test repository.

```rust
assert!(wrong_context_start.is_err());
assert!(gateway.mutations().is_empty());
assert_eq!(persisted_job.status, ScopedJobStatus::Blocked);
assert_eq!(persisted_job.next_batch, 1);
```

Use the gateway's existing test mutation ledger (or a test utility wrapper) rather than adding production inspection APIs.

- [x] Run all service/engine/persistence/provider tests and Clippy in Docker; commit the scoped compatibility engine.

---

### Task 6: Install the production v2 application/IPC boundary and finish backend lifecycle coverage

**Files:**
- Create: `src-tauri/src/provider_service.rs`, `src-tauri/src/compatibility/mod.rs`, `model_v2.rs`, `commands_v2.rs`, `tests.rs`.
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/foundation_lifecycle_tests.rs`, `src-tauri/src/error.rs`.

**Interfaces:**
- Consumes: registry, scoped compatibility provider, v3 state, active context, common envelope decisions.
- Produces: `ProviderService` implementing snapshot/bootstrap/search/refresh/prepare/authorize/execute/jobs/cancel using neutral refs. Register commands with `_v2` suffix for all identity-bearing operations. Auth/settings remain typed, but settings replacement returns a v2 bootstrap/snapshot.

- [x] Add RED command tests for contract version mismatch, stale generation, wrong account/source/provider, missing identity, invalid application ID/canonical reference, and no fallback to old commands. Exercise exported command handlers through a test runtime state rather than source-text scans.

```rust
let response = api.prepare_selection_v2(wrong_account_request).await;
assert_eq!(response.unwrap_err().code, "scope_mismatch");
assert!(gateway.mutations().is_empty());
```

- [x] Build bootstrap separately from active-context operations. Bootstrap can show setup/auth/identity progress and legacy historical rows without fabricating an account. Identity failure exposes safe retry. Production startup migrates the store and verifies identity before creating/resuming a scoped executor.

- [x] Validate common envelopes centrally and dispatch through the registry. Search/refresh resolve only explicit scoped refs; a dirty refresh never calls the catalog listing path. Validate provider results before exposing them. Safe command errors contain codes/messages, not arbitrary native strings.

- [x] Replace the production Tauri registration list and runtime state. Remove v1 destructive entry points from production registration; do not keep an unscoped fallback. Coordinate settings shutdown, worker completion, shared store lock ownership, new binding, and runtime replacement without enabling jobs on an unverified identity.

- [x] Rewire Task 1 backend tests to the real v2 entry points, preserving the expected values/behavior. Extend the lifecycle test to resolve a nonnumeric synthetic provider through registry, prepare a scoped plan, round-trip a job through v3, reload/recover safely, and produce scoped dirty refs. Unknown native numeric assumptions must fail this test.

- [x] Run complete backend, neutral-domain, and legacy-domain suites plus Clippy. Verify no v2 response leaks auth secrets or message content into durable state. Commit the active backend boundary.

---

### Task 7: Cut the frontend and fixture adapter over to scoped string identities

**Files:**
- Create: `src/providers/contract.ts`, `src/providers/identity.ts`, `src/test/legacy-types.ts`, `src/providers/contract.test.ts`.
- Modify: `src/types.ts`, `src/api-contract.ts`, `src/api.desktop.ts`, `src/api.fixture.ts`, `src/demo.ts`, `src/ipc-contract.test.ts`, `src/api.test.ts`, `src/App.tsx`, `src/components/ResultsList.tsx`, `Sidebar.tsx`, `SearchToolbar.tsx`, `ConfirmDialog.tsx`, `src/provider-lifecycle.test.tsx`, affected component tests.

**Interfaces:**
- Consumes: the v2 envelopes/records from Task 6 and shared fixture.
- Produces: branded string application IDs and validated scope/ref DTOs; `scopeKey(scope)`, `resourceKey(ref)`, and `sameContext(left,right)` using tuple encodings; `RetractApi` migrated to v2 without numeric fallback.

- [x] Add RED runtime decoder tests for version mismatch, malformed/missing context, numbers in ID fields, conflicting locator/application identity, and optional/null field handling. Add TS type assertions for exact Rust/TypeScript request/response shapes. Keep the frozen v1 JSON/types as historical tests in their own namespace.

- [x] Implement small validators without adding a schema library. Decode before publishing snapshot/search/job data. Resource identities are opaque branded strings, not parsed Telegram values. Avatar rendering uses an explicit numeric seed or stable string hash, never `senderId % 19`.

```ts
export const scopeKey = (scope: Scope): string =>
  JSON.stringify([scope.provider, scope.accountId, scope.sourceId]);
export const resourceKey = (ref: ScopedResourceRef): string =>
  JSON.stringify([ref.scope.provider, ref.scope.accountId, ref.scope.sourceId,
    ref.resource.resourceKind, ref.id]);
```

- [x] Update desktop dispatch to the real `_v2` commands and include expected context in every active operation. Bootstrap/settings keep context optional until verified. Copy needed normalized refs from backend results; do not synthesize native IDs from UUIDs or inspect locator payloads.

- [x] Migrate fixture records to explicit deterministic UUIDs/scopes and serialized descriptor data. Fixture-only code may simulate operations for existing tests, but must obey the same v2 contract and remain excluded from production. Do not add an end-user demo mode or a second production policy engine.

- [x] Update selection, album grouping, chat lookup/filtering, prepare requests, polling, and pending-removal sets to scoped keys. Context changes clear incompatible review/selection state and increment all response generations. Keep loading blocked until identity and catalog are ready, with progress/error/retry feedback.

- [x] Complete Task 1 frontend tests against actual v2 UI/adapter paths. Add stale search, dirty-refresh, job-poll, and settings-response tests for cross-account/source changes. Assert only affected matching-scope conversations refresh, no global snapshot/counter reset for individual cleanup, and no hidden selection widening.

- [x] Run all Vitest tests, TypeScript/build, and production-bundle checks in Docker. Both Task 1 RED suites must now be GREEN; do not delete or skip the regression tests. Commit the frontend cutover.

---

### Task 8: Render backend action descriptors and accessible complete job outcomes

**Files:**
- Create: `src/components/JobDetails.tsx`, `src/components/JobDetails.test.tsx`.
- Modify: `src/components/ImpactPanel.tsx`, `src/components/ImpactPanel.test.tsx`, `src/components/ConfirmDialog.tsx`, `src/App.test.tsx`, `src/styles.css`, provider fixture descriptor records.

**Interfaces:**
- Consumes: backend action descriptors, ordered plan steps, scoped jobs and safe diagnostics from Tasks 5–7.
- Produces: descriptor-driven visibility/disabled state/confirmation/effect copy; on-demand accessible job detail rendering.

- [x] Write RED UI tests where the displayed admin label and executable descriptors disagree. The descriptor controls the action, not the label/provider name. Test compound cleanup/leave effects, unavailable reasons, protected messages, and standalone critical group destruction.

- [x] Render the existing authority/impact interface from descriptors while preserving familiar Telegram presentation. Map descriptor kinds/effects to icons and layouts only; do not infer permission. ConfirmDialog summarizes every ordered effect and uses backend confirmation requirements.

- [x] Write RED job-detail tests with deleted `2`, skipped `3`, failed `4`, a retry countdown, a blocked-account reason, and `migration_requires_new_review`. Assert distinct accessible labels and that unsafe raw provider text is never rendered. Preserve the compact current recent-job row and add a disclosure/button for details.

```tsx
fireEvent.click(screen.getByRole("button", {name: /Job details/}));
expect(screen.getByText("Skipped")).toBeVisible();
expect(screen.getByText("Failed")).toBeVisible();
expect(screen.getByText(/Review this cleanup again/)).toBeVisible();
expect(screen.queryByText("SYNTHETIC_RAW_PROVIDER_SECRET")).not.toBeInTheDocument();
```

- [x] Implement the disclosure/dialog with correct focus/labels, existing visual tokens, and safe code-to-message mapping. Blocked jobs offer truthful account/review guidance without preventing connection configuration or promising that in-flight work was undone.

- [x] Run all component/App tests, TypeScript/build, and bundle checks in Docker. Commit descriptor-driven presentation and outcome details.

---

### Task 9: Complete regression, integration review, and migration documentation

**Files:**
- Modify: `docs/TELEGRAM_CHARACTERIZATION.md`, `docs/TEST_PLAN.md`, `docs/LOCAL_LIVE_TEST.md`, `README.md`, `PLAN.md`, `CHANGELOG.md`.
- Test: all changed crates/components, `scripts/check-public-repo.mjs`, release metadata tests, production-bundle verifier, existing CI workflows.

**Interfaces:**
- Consumes: completed scoped production path and RED/GREEN evidence from Tasks 1–8.
- Produces: a fully verified branch, documented upgrade behavior/limits, and review evidence. No tag/release/remote merge.

- [x] Add any missing regression test exposed by integration before its fix. Check startup with no profile, both legacy fixture versions, malformed state, interrupted migration, identity verification failure, renamed account, same-account restart, wrong account, cancellation/rate waits, and save failure between batches. Task 9 additionally covers actual child-process lock contention and invalid v3 with valid legacy backups; both already passed before any production change. Existing persistence fixtures exposed canonical-tag/order drift and were corrected without weakening production or historical checks.

- [x] Run all required Docker gates:

```bash
npm run container:check
docker buildx build --platform linux/amd64 --target checks --output type=cacheonly --progress plain .
docker buildx build --platform linux/arm64 --target checks --output type=cacheonly --progress plain .
```

Use the native macOS packaging workflow for the macOS-only artifact gate when available; never claim a container made a macOS application. Do not start real Telegram cleanup or read real keychain/session data to satisfy a test. Report any unavailable native/live gate precisely.

Task 9 evidence (2026-09-04): default/native-architecture Docker plus explicit amd64 and arm64 checks pass with 150 frontend, 168 backend, 19 neutral-domain, 17 legacy-domain and 7 release tests, along with TypeScript/build/bundle exclusion/public metadata/fmt/Clippy. Native macOS packaging and live Telegram were unavailable/not run; rendered browser/native short-viewport and accessibility checks remain manual.

Final-review fix-wave evidence (2026-09-04): actual registered setup/failed bootstrap serialization now feeds frontend onboarding/recovery tests; failed settings replacement is rediscovered without replaying mutations; explicit failed-connection recreation preserves invalid-v3/legacy-backup bytes; own-message descriptors respect known active-group restrictions. Focused regressions pass (5 backend review cases, expanded invalid-v3 case, 36 frontend recovery/lifecycle/settings cases). Final default, explicit amd64, and explicit arm64 Docker gates pass with 158 frontend, 173 backend, 19 neutral-domain, 17 legacy-domain and 7 release tests plus all previous build/bundle/metadata/fmt/Clippy gates. The initial amd64 full run exposed a pre-existing test scheduling race; its isolated pass and deterministic initial-authorization synchronization are recorded, with delayed-identity/signout assertions and production identity logic unchanged. Scoped re-review confirmed all four findings addressed with no new issues. The controller independently reran the full Docker gate on committed `6207734` (build `mgabqc4obu0xtwxp10dy8n32w`), with all tests and checks passing. Native/live/manual limitations remain as above.

- [x] Update migration docs: preserved `jobs.pre-provider.enc`, no automatic restoration, old binaries reject v3, unfinished old jobs require fresh review, blocked wrong-account work does not retarget, credentials/session paths unchanged, and messages are not indexed/persisted by this stage. Update the Telegram contract to distinguish frozen v1 reader coverage from current v2 UI behavior.

- [x] Verify new crate lock/version/license updates and unsigned release tooling. Run `node scripts/check-public-repo.mjs`, release metadata tests in Docker, `git diff --check`, and production bundle checks. Ensure all SDD scratch/test providers stay out of production artifacts. Linux metadata/bundle checks pass; native package inspection is separately unrun.

- [x] Commit completion docs, request whole-branch code review, address findings through reviewed fixes, and report actual test results/remaining gates. Keep the branch available for the user's integration decision; do not merge or publish automatically.
