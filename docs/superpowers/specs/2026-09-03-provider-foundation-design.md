# Provider Foundation: Versioned Compatibility Bridge

**Date:** 2026-09-03

**Status:** Approved for implementation on 2026-09-03

**Base:** `ef5f1f62830df6f508bb7a13c2dc4c8868cb0977` — merged Telegram characterization gate, PR #20

**Parent design:** [Multi-provider architecture](2026-09-01-multi-provider-architecture-design.md)

**Regression contract:** [Telegram behavior contract](../../TELEGRAM_CHARACTERIZATION.md)

## 1. Decision and scope

Introduce a versioned compatibility bridge that puts scoped, opaque identities on real application paths while retaining the tested Telegram implementation underneath. The user approved this approach on 2026-09-03.

This stage produces provider-neutral domain contracts, a provider registry, an account-aware application boundary, a versioned IPC contract, and authenticated plan/job persistence. The existing Telegram UI uses the new identity contract; it does not continue calling an unscoped numeric API beside an unused provider scaffold.

The bridge is transitional. It wraps the current Telegram search, planning, authorization, and execution behavior; it does not replace TDLib, reimplement Telegram policy, or relocate the entire legacy service. The later Telegram-provider migration removes that legacy implementation dependency behind the established contracts.

### Alternatives considered

| Approach | Benefit | Cost / limitation |
| --- | --- | --- |
| Versioned compatibility bridge — selected | Exercises scoped identities in real workflows while preserving tested Telegram behavior. | Requires explicit translation and a temporary legacy boundary. |
| Additive domain scaffold | Small initial patch and little production risk. | Does not prove selection, persistence, execution, or refresh migration. |
| Full provider migration now | Removes the temporary boundary immediately. | Combines ID, service, persistence, and TDLib changes in a substantially larger review. |

### Included

- Provider/account/source identity and normalized records.
- Opaque conversation, content, actor, and grouping identifiers across Rust, IPC, and TypeScript.
- Registry and focused query, connection, remediation, and import contracts.
- Backend-owned action descriptors, explicit effects, and structured errors.
- A production version-2 compatibility API for the existing Telegram interface.
- Account-bound plans, authorization grants, jobs, and targeted reconciliation.
- A content-free version-3 encrypted state format and safe legacy job migration.
- Test-first lifecycle coverage and preservation of the Telegram regression gate.

### Excluded

- SQLCipher, FTS, content indexing, or archive ingestion.
- Discord/X providers, OAuth, paid API access, or token handling.
- A multi-account switcher, mixed-provider search, or provider UI redesign.
- Moving or rewriting TDLib profiles, authentication, catalogs, or search.
- Namespaced credential-vault migration or new keychain entries.
- Release tags, signing requirements, telemetry, or hosted infrastructure.

## 2. Boundaries and ownership

`retract-domain` is a new pure Rust crate for provider-neutral identities, normalized records, descriptors, plan envelopes, and structured errors. It must not depend on Tauri, TDLib, network clients, filesystem access, or provider-specific policy.

`src-tauri/src/providers` owns the provider registry and focused ports. Registration is explicit; lookup uses a validated `ProviderKey`. Core services dispatch through registered implementations, not comparisons such as `provider == "telegram"`.

`src-tauri/src/providers/telegram` owns typed Telegram locators, native numeric conversion, authenticated identity discovery, normalization, action translation, and compatibility calls to the existing engine. The existing `cleaner-domain`, `LiveGateway`, and `CleanerService` remain the legacy Telegram implementation during this stage. Importing their native types outside this named compatibility boundary is not a pattern for new providers.

A provider application service owns active context, version-2 command routing, and scope validation. The existing execution engine retains its mutation ordering and rate-limit behavior. It consumes the same scoped plan fingerprint that the application reviews and authorizes; there must not be separate outer and inner grants authorizing different targets.

Only one state-store writer exists for a profile. Legacy serialization remains a reader/translator, not a second writer beside the new store.

## 3. Identity model

### Application identities

Use validated `ProviderKey` strings and distinct UUID-backed Rust newtypes for `AccountId`, `SourceId`, `ConversationId`, `ContentId`, and `ActorId`. Their JSON representations are strings. TypeScript uses distinct branded string types with boundary validation, not `number | string` unions.

An active context contains:

```text
Scope { provider, accountId, sourceId }
ActiveContext { scope, sessionGeneration }
```

`sessionGeneration` is an opaque, ephemeral identifier for the authenticated runtime instance. It invalidates stale requests and grants; it is not a substitute for durable account identity or a persisted credential.

### Verified account identity

The Telegram adapter establishes identity only after an authenticated `getMe` result supplies a valid native account ID. Authentication being reported as ready is insufficient by itself: current code sets the ready state before that request completes.

The adapter defines a typed identity locator containing the Telegram environment (`production` or `test`) and the native account ID as a decimal string. Display names, phone input, profile-directory names, and frontend-supplied account IDs never establish account ownership.

The encrypted state retains a stable mapping from the validated provider identity locator to an application `AccountId`. The first verified encounter creates that UUID; later sign-ins to the same provider/environment/account reuse it. A live source receives a stable `SourceId` associated with that account and profile. Renaming the account does not change these IDs. Another account using the same TDLib profile gets different IDs.

New IDs are persisted successfully before they can be returned as a usable active context. Failed persistence leaves cleanup unavailable with a retryable, safe error; it must not return a transient identity that changes on restart.

### Resource identity and locators

Retain the parent design's `ProviderResourceRef` envelope:

```text
provider, accountId, resourceKind,
locatorSchema, locatorVersion, canonicalKey, locatorPayload
```

Source provenance is explicit beside the resource reference; it is not inferred from a provider name. Provider-owned locators serialize native IDs as strings, including Telegram's signed chat IDs, message IDs, sender IDs, and album/grouping IDs.

The owning adapter validates the locator schema/version, resource kind, field types, native ranges, canonical key, and scope. Unknown versions and inconsistent envelopes are rejected. For Telegram, conversion to `i64` happens only inside its adapter/legacy boundary. Core and frontend code may retain and return locator envelopes, but must not interpret their fields.

Canonical keys use deterministic, unambiguous encodings of typed identity tuples. Telegram message keys include chat and message IDs; actor keys distinguish user senders from chat senders. Concatenation with an unescaped delimiter is insufficient.

Conversation/content/actor application UUIDs are derived using standard UUID v5 with the persisted account UUID as namespace and a versioned, kind-tagged canonical tuple as name. This avoids persisting an unbounded mapping for every search result. The full canonical locator remains authoritative: reject an application-ID collision with a different canonical reference rather than merging the records. These UUIDs are identifiers, not authorization tokens or anonymization guarantees.

For one provider/account, the same remote resource has the same application ID across live and eventual archive sources. Provenance remains separate. The foundation UI has one active source; it rejects requests that combine sources. Later mixed-source search will explicitly handle deduplication and plan partitioning.

### Selection identity

Selection keys include provider, account, source, resource kind, and application resource ID using an unambiguous tuple encoding. Album grouping also includes conversation and active scope. Matching album IDs in another conversation/account must never expand a selection.

Neither keys nor native numeric-looking strings pass through `Number`, `parseInt`, floating-point arithmetic, or numeric JSON serialization. Numeric values such as counts, timestamps where already numeric, and avatar seeds are not identifiers and need not change.

## 4. Normalized records and provider ports

Use the account/source/conversation/content/attachment fields and taxonomies in the parent design. `photo` maps to normalized `image`; `file` maps to `document`; Telegram-specific kinds and presentation labels remain typed adapter metadata. Preserve information needed by current filters rather than collapsing secret chats, pin state, albums, or outgoing-message status.

Records carry provider/account/source identity, an observation timestamp, and live/archive evidence. Query results may contain searchable text and inert attachment metadata in memory. This stage persists neither message bodies nor an archive content catalog.

The provider contracts remain focused:

- `QuerySource`: list conversations, search, resolve explicit references.
- `LiveConnection`: provider-owned connection flow and verified account/context state.
- `RemediationProvider`: action discovery, live preflight, bounded execution, verification.
- `ArchiveImporter`: inspection/import contracts only; no production archive importer is registered in this stage.

The registry exposes supported ports and static integration modes. Static capabilities never authorize a particular target. An absent port is unavailable, not a stub that returns success.

Telegram query/connection/remediation wrappers delegate to characterized behavior. Required extraction of an existing helper is permitted with before/after regression evidence. Full service decomposition and removal of the compatibility boundary remain the later Telegram migration. A synthetic provider, registered only in tests, proves that core contracts and scope handling do not assume Telegram-shaped identifiers.

## 5. Version-2 application and IPC contract

Create explicit version-2 commands for identity-bearing operations: snapshots, search, targeted refresh, plan preparation, authorization, execution, job listing, and cancellation. Each request/response has a fixed contract version; active operations include expected scope and session generation.

The `@retract/api` alias remains. The desktop adapter and screenshot/test adapter both implement the version-2 contract. Existing UI method names can be retained where their meaning is unchanged, but their DTOs become scoped and string-identified. There is no frontend conversion back to numeric Telegram IDs.

Telegram-specific auth submissions and connection settings remain typed provider operations with unchanged user-facing setup. Settings replacement returns the new version-2 context rather than a legacy numeric snapshot.

Do not register unscoped version-1 destructive commands alongside the new commands. An outdated frontend receives a contract-version/reload error, never a fallback to the old executor. Historical version-1 DTOs and fixtures remain in compatibility tests/readers; keeping them readable is not permission to execute through them.

The backend checks expected active context before resolving resources and before accepting plan/authorization/execution requests. Unknown account/source combinations, stale generations, invalid locators, and unsupported contract versions fail closed with structured errors.

The main UI remains blocked during initial identity/catalog preparation, preserving the user's loading preference. Existing catalog counters remain visible. Identity establishment adds a truthful stage such as “Verifying Telegram account”; failure offers retry rather than an indefinite spinner. Account identity lookup must not trigger a second global catalog refresh.

## 6. Plans, effects, and authorization

### Reviewed scope

An executable plan contains one provider, one account, and, for this stage, one source. Each target carries its versioned canonical locator and application identity. The backend resolves requested references and creates the frozen plan; it never accepts client-supplied permissions, effects, or an executable recipe.

Selected-message operations freeze exact reviewed IDs. Broad container operations retain their explicit native semantics, not a false claim that every future affected message was individually frozen. Their container target, requested operation, policy, confirmation, and expected effects are immutable.

Compound Telegram cleanup/leave intents expand into ordered reviewed steps. Each step has one action kind and explicit expected effect; message cleanup, membership removal, and self-removal remain distinguishable. A combined button must not hide cleanup scope behind a membership-only effect. Permanent group destruction stays a separate critical action.

### Fingerprint

Introduce an explicitly versioned SHA-256 fingerprint over a deterministic canonical representation containing:

- plan ID and fingerprint schema version;
- provider/account/source scope;
- ordered action steps and expected effects;
- locator schemas/versions, canonical keys, and immutable targets;
- provider execution-recipe schema and payload, excluding the fingerprint field itself;
- confirmation tier, exact confirmation data, and applicable batch/restart policy.

Canonicalization uses typed structures and recursively ordered object keys; input map insertion order cannot change a fingerprint. Collection ordering is specified per field: unordered target sets are sorted/deduplicated by canonical identity, while execution-step order is preserved.

The Telegram compatibility layer applies the scoped fingerprint before a plan is persisted or shown. Existing legacy confirmation and grant code must use that same fingerprint. There is no period in which an unscoped plan can execute, and no double-prompt bridge that obtains separate authorizations for outer and inner plans.

Unknown recipe schemas, a recipe/target mismatch, a changed effect, or a changed account/source invalidates the plan. Provider payloads are typed, content-free execution data, not arbitrary message/metadata blobs.

### Live execution

Keep the existing review, acknowledgement, exact-title confirmation, 60-second single-use owner grant, bounded calls, per-message capability checks, rate-limit handling, cancellation, and cleanup-before-leave ordering.

The grant additionally binds the active session generation and scope. Recheck active context after an asynchronous owner-authentication prompt returns and immediately before consuming its grant. A context switch while the prompt is open cannot create a grant for the new context.

Before each provider call, and after every rate-limit wait, verify the worker's account/source binding still matches a live session and perform the required live authority check. Native requests use the session handle captured for that verified binding; the handle must not be rebound to another account behind an active worker. Sign-out/context replacement invalidates grants and later calls. Already in-flight outcomes may be uncertain and must not be reported as cancelled deletions or confirmed success without evidence.

Durably persist job creation before the first mutation, and acknowledged progress before advancing the recoverable cursor. If a save fails, stop scheduling further mutations and show a safe persistence error. Do not reuse the current `resume_incomplete` behavior that ignores a failed persistence result when sealing recovery decisions.

## 7. Encrypted state and migration

### State format

Continue using the existing encrypted job-store key and AES-GCM implementation. Introduce header `RTRCT03` with associated data that includes format/schema, provider/profile identity, and the store's scope. This foundation store remains profile-based and can contain multiple historical account identities; each executable row separately binds provider/account/source through its authenticated plan and validated relationships. A future account-specific store must additionally bind that account in associated data.

The version-3 state contains:

- stable account and source identity metadata;
- scoped frozen plans and content-free provider execution recipes;
- scoped jobs, counters, retry/cursor data, and safe diagnostics;
- explicitly unbound legacy historical records;
- migration provenance identifying the legacy source digest and format.

Do not persist message bodies, auth values, tokens, arbitrary provider errors, archive paths, or sensitive attachment names. Confirmation titles/names already required by the existing frozen-plan workflow remain protected plan data, not diagnostic logs.

Keep TDLib files, connection-settings schema, vault format, and existing keychain entries unchanged. No extra credential read is needed per account lookup, command, or job; reuse the existing in-process vault cache.

### Single-writer rule

Hold a profile-level interprocess exclusive store lock before migration or writes. Share its ownership within the process rather than opening competing writers during runtime replacement. Another application process receives a clear “profile already in use” error. The existing settings transition must stop its worker/service before transferring store ownership.

The new writer cannot call the legacy `PersistedState` serializer directly into `jobs.enc`. Compatibility views of plans/jobs use the same version-3 store transaction boundary.

Serialize state mutations and their writes together, not just file replacement. Concurrent workers must not save stale snapshots over a newer cursor or account mapping. A failed transaction is not published as successful in-memory state, and no later request may execute on the assumption that an unsuccessful save was durable.

### Migration sequence

Migration runs before constructing any resumable executor:

For a genuinely new profile with neither `jobs.enc` nor migration artifacts, initialize an empty version-3 state through the same private write/verify/replace path. If a backup or migration artifact exists but the active file is missing, stop for recovery rather than silently creating empty history. Setup-only mode creates no verified account, scoped executable job, or live identity registry using its existing fixed test/setup key.

1. Lock the profile and authenticate/read `jobs.enc`. Version-3 input is validated directly. Unrecognized/corrupt input stops startup cleanup; there is no empty-state or legacy fallback.
2. For `RTRCT01`/`RTRCT02`, preserve the exact original ciphertext in `jobs.pre-provider.enc` using exclusive creation. If that backup already exists, require its bytes/digest to match the authenticated migration source; never overwrite a different backup.
3. Sync the backup and containing directory. Build the new state in memory without assigning an account to old jobs. Preserve terminal history. Stop every nonterminal legacy job as failed/partial according to its existing completed count, retain its counters, clear retry scheduling, and add `migration_requires_new_review`.
4. Write version 3 to a separate private temporary file. Decrypt/read that actual file and validate schema, relationships, retained-history counts, stopped-job policy, and migration provenance before switching.
5. Atomically replace `jobs.enc` only after verification; sync the containing directory. Retain `jobs.pre-provider.enc`. The backup is not executable state and is never automatically restored.
6. Start version-3 services. Resolve/record the current account only after successful live identity verification. New work uses that binding; old rows remain “Legacy Telegram history — account not verified.”

Using the new header at the current `jobs.enc` path also causes older Retract versions to reject the file instead of replaying the preserved legacy jobs. Do not leave the old resumable file at its active path beside a differently named new store.

On interruption before replacement, the original remains the active file and the new version retries the guarded migration before scheduling anything. On interruption after replacement, the authenticated version-3 file is authoritative. An invalid version-3 file fails closed even if a valid backup exists. Unexpected leftover temporary files are not used as active state or executed.

This is a migration-preservation guarantee, not a general rollback/backup product. Document the backup's purpose and never remove or restore it automatically.

### New-job recovery

Version-3 jobs may resume automatically only when durable authorization/start provenance exists, the adapter explicitly classifies the frozen operation as safe to retry, the persisted cursor is valid, and live verification establishes the exact same account/source before any call. Recheck target authority before resuming and after waits.

Broad, dynamic, cost-bearing, unsupported-schema, and ambiguous operations require new review. An otherwise resumable job whose account is unavailable or different enters an explicit `blocked` status with a safe structured reason, preserving its cursor but scheduling no worker. It is not attached to the current account. A blocked job does not prevent the user from changing connection settings to establish the correct identity; it is distinct from an active worker. Only the original verified scope can make it eligible for recovery again.

If an account change interrupts a call whose outcome is unknown, use the ambiguous-outcome/new-review path instead of treating the job as safely blocked. Terminal legacy rows remain visible but have no executable retry action.

## 8. Frontend behavior and targeted reconciliation

Keep the existing three-pane layout and Telegram setup. Scope changes invalidate outstanding search/refresh generations, clear incompatible selections and review state, and prevent previous-scope job updates from mutating the new view.

Dirty references and completed-job impacts contain explicit scope plus conversation/content identities. Refresh only affected conversations within the current matching scope. Do not reload all chats to resolve one ID, verify one action, or remove one finished chat.

Hidden selection counts and album atomicity continue working. Stable application IDs preserve row identity across refreshes; changes to display names or observed timestamps do not create new identities. A removed chat remains visibly pending until its result is known, and repeated execution of the same plan remains rejected.

Backend action descriptors drive allowed cleanup buttons, effect copy, confirmation, and unavailable reasons on the migrated interface. Presentation may use Telegram labels provided by the adapter, but cannot infer authority from an admin label or provider name.

Because provider-owned result descriptors reach the real UI here, include the parent design's prerequisite accessible job detail presentation: deleted, skipped, failed, retry state, and safe structured diagnostics. Preserve the compact existing recent-job row and expose details on demand; do not replace it with a dashboard redesign. Add explicit text for migrated jobs requiring a new review.

## 9. Error model and privacy

Retain the parent design's provider error kinds: `authentication_required`, `permission_changed`, `not_found`, `already_removed`, `rate_limited`, `cost_limit_reached`, `transient`, `permanent`, `ambiguous_outcome`, `unsupported_schema`, and `invalid_archive`.

Application-boundary errors separately identify `unsupported_contract_version`, `scope_mismatch`, `stale_context`, `identity_unavailable`, `profile_in_use`, and `state_persistence_failed`. Migration/restart diagnostics include `migration_requires_new_review` and `restart_requires_new_review`. Provider-native error strings are translated inside the adapter, never copied into normal UI or durable jobs.

Error DTOs carry stable codes, safe predefined messages, retry timing when applicable, and no arbitrary details bag. Retryability never permits changing the reviewed effect or expanding targets. Result counters distinguish confirmed deletions, skipped/unavailable targets, failures, and uncertain outcomes; “request accepted” is not universally equivalent to “deletion verified.”

## 10. Test-first acceptance gate

Before changing any production identifier type, add RED behavior-level tests against real application/bridge paths with synthetic references including:

- the distinct strings `9007199254740992` and `9007199254740993`;
- a nonnumeric locator such as `message:part/0007`;
- negative Telegram chat IDs;
- identical native IDs across two providers, two accounts, and two conversations;
- matching grouping IDs in different conversations;
- unknown locator versions and canonical-key/payload disagreement.

The nonnumeric provider is test-only. It must traverse shared selection, IPC, plan-envelope, store, and reconciliation code; it must not be a separate implementation that bypasses the production lifecycle.

| Boundary | Required assertions |
| --- | --- |
| Selection | Scoped items cannot collide; album and hidden-selection behavior is preserved. |
| IPC | Shared Rust/TypeScript fixtures and exact field/enum/optionality checks; IDs remain strings without numeric conversion. |
| Identity | Same verified account survives restart/rename; different accounts/environments do not collide; failed `getMe` or identity save cannot enable cleanup. |
| Planning | One scope, immutable targets/steps, deterministic fingerprint; changing any bound field invalidates authorization. |
| Owner authorization | Context changes during the prompt and after approval reject the grant; one prompt and one single-use scoped fingerprint. |
| Execution | Account mismatch, permission loss, cancellation, and rate waits cannot expand scope or trigger fallback. |
| Persistence | Actual AES-GCM file round trips preserve opaque identities; tamper/wrong-profile fail; failed writes prevent later calls. |
| Migration | Frozen v1/v2 inputs remain readable; interrupted backup/write/verify/replace stages preserve recoverability; all old nonterminal jobs stop; no automatic backup fallback. |
| Recovery | Only eligible v3 frozen jobs resume under the same verified account/source; cursor bounds and ambiguous outcomes stop unsafe replay; blocked jobs permit connection changes without retargeting. |
| Reconciliation | Only dirty matching-scope conversations refresh; stale results cannot clear or replace another context. |
| Job details | Accessible skipped/failed counters and safe diagnostics, including migration-required review, without raw provider strings. |
| Bundle isolation | Synthetic providers and screenshot adapters are absent from production assets. |

Keep historical version-1 numeric fixtures as compatibility evidence. Do not rewrite them to look like version 2 or cite them as opaque-ID coverage. Keep the characterized native TDLib call sequences and mutation ordering unless a separately documented safety-boundary change here requires a new assertion.

Full validation uses the pinned Docker BuildKit checks on amd64 and arm64 and the native macOS package job. Use `--output type=cacheonly`, named caches, non-root/offline project execution, and no global Docker prune. Real destructive parity checks use only the disposable test-DC matrix, with explicit authorization to operate those identities; automated synthetic coverage does not claim a live test was run.

## 11. File responsibilities and delivery order

| Area | Planned location / responsibility |
| --- | --- |
| Neutral domain | `crates/retract-domain/src/identity.rs`, `records.rs`, `actions.rs`, `plans.rs`, `error.rs`; re-export through `lib.rs`. |
| Provider registration | `src-tauri/src/providers/registry.rs`, `ports.rs`, `mod.rs`; explicit implementations and focused contracts. |
| Telegram bridge | `src-tauri/src/providers/telegram/identity.rs`, `locators.rs`, `compat.rs`, `mod.rs`; native interpretation and delegation. |
| Versioned boundary | `src-tauri/src/provider_service.rs`, `src-tauri/src/compatibility/model_v2.rs`, `commands_v2.rs`, `mod.rs`; scope-aware application entry points. |
| Persistence | `src-tauri/src/persistence/foundation_store.rs`, `migration.rs`, `mod.rs`; version-3 single-writer state and legacy conversion. Existing `secure_store.rs` retains credential handling and reusable authenticated I/O. |
| Legacy engine | Existing `service.rs`, `gateway.rs`, `model.rs`, and `cleaner-domain`; only bridge/fingerprint/store/session seams required now. |
| Frontend contract | `src/types.ts`, `src/api-contract.ts`, `src/api.desktop.ts`, and `src/api.fixture.ts`; migrate atomically to version 2. |
| UI identity and results | `src/App.tsx`, `src/components/ResultsList.tsx`, `Sidebar.tsx`, `ImpactPanel.tsx`, `ConfirmDialog.tsx`; scoped keys, descriptors, targeted refresh, accessible job details. |
| Regression evidence | Extend `src/ipc-contract.test.ts`, `src/App.test.tsx`, backend service/store tests, and new provider contract/lifecycle fixtures. |

The implementation plan will follow these independently reviewable slices:

1. RED lifecycle harness and fixture contract, before production ID changes.
2. Neutral domain, registry, typed locators, verified account context, and deterministic resource identities.
3. Version-3 store, legacy migration, scoped fingerprints, and guarded recovery.
4. Version-2 backend commands and Telegram compatibility execution path, including scoped authorization and save-failure handling.
5. Frontend cutover, action/result presentation, and scope-aware reconciliation.
6. Full regression, production-bundle, container, native-package, and documentation gates.

If splitting PRs, the first production identity cutover must include the corresponding passing lifecycle evidence; do not merge a partially migrated execution boundary. Preparatory contract-only slices are not a completed foundation stage.

The new crate requires its own lockfile while the repository has no root workspace, plus the `src-tauri` path dependency/lock update. Include it in Docker fetch/test/fmt/Clippy, npm checks, Dependabot directories, and public/release version/license checks. No toolchain upgrade or unrelated dependency maintenance is part of this work.

## 12. Specification review checklist

- [x] Preserve the selected hybrid architecture and separate archive/Telegram-migration stages.
- [x] Put opaque identities on actual UI, plan, job, store, and reconciliation paths.
- [x] Resolve verified account identity, stable IDs, session changes, and native-locator ownership.
- [x] Define one versioned execution/authorization boundary without a legacy IPC bypass.
- [x] Define backup/write/verify/replace migration and conservative old-job recovery.
- [x] Include the accessible job-result prerequisite when migrating action/result descriptors.
- [x] Preserve keychain/session behavior and container cache discipline.
- [x] User review of this written specification.
- [x] Write the detailed implementation plan after specification approval: [Provider foundation implementation plan](../plans/2026-09-03-provider-foundation.md).

No production implementation is included in this document change.
