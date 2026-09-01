# Retract Multi-Provider Architecture Design

**Status:** Approved

**Approved:** 2026-09-01

**Date:** 2026-09-01

**Scope:** Phase 0 architecture only; this document does not authorize or implement the refactor
**Recommended approach:** Hybrid provider architecture with live Telegram access and an encrypted local archive index

**First implementation plan:** `docs/superpowers/plans/2026-09-01-telegram-characterization.md`

## Purpose

Retract should find privacy-sensitive content across communication platforms, remove it where a platform permits, and assist the user where automatic removal is unavailable.

Telegram, Discord, X, and future sources are provider integrations. They must not become separate application architectures or leak provider-specific behavior throughout the core UI and services.

This design evolves the current Telegram application incrementally. It preserves Telegram's working TDLib behavior, adds a normalized provider boundary, and introduces local archive indexing only where it is needed for Discord, X, and future archive providers.

## Product principles

- Retract remains free, open source, local-first, and usable without hosted Retract infrastructure.
- Existing Telegram behavior and locally stored Telegram sessions survive the migration.
- Provider capabilities are explicit data, never assumptions inferred from a provider name.
- The Rust backend remains the safety and authorization boundary.
- Imported content and privacy analysis remain local.
- Message bodies, sensitive attachment names, and credentials never enter job or diagnostic logs.
- No Discord self-bot, copied user token, browser-session extraction, or unofficial normal-user automation is permitted.
- Discord archive support must remain useful even when automatic deletion is unavailable.
- X archive support must remain useful without paid API access.
- Every destructive operation continues through review, immutable plan creation, fresh authorization, execution-time preflight, bounded execution, and truthful result reporting.
- Retract must not silently substitute a weaker or different remediation effect.
- Facebook and other future providers are design tests, not implementation scope.

## Repository audit

### Technology and application boundary

Retract is a Tauri 2 desktop application. React 19, TypeScript, Vite, and Vitest implement the frontend. Rust 2024 implements the trusted application service, TDLib adapter, secure persistence, and deletion-plan domain.

The current data flow is:

```text
React UI
  -> hand-written RetractApi contract
  -> Tauri IPC commands
  -> CleanerService
  -> TelegramGateway
  -> LiveGateway
  -> TDLib JSON client
  -> Telegram
```

The production frontend adapter and screenshot/test fixture are selected through the `@retract/api` Vite alias. This is a useful seam and should remain.

### Current backend responsibilities

`CleanerService` currently owns:

- application bootstrap snapshots;
- Telegram search and targeted chat refresh;
- frozen plan creation;
- confirmation proof validation;
- single-use, plan-bound macOS owner-authentication grants;
- job creation, execution, retry, cancellation, and restart behavior;
- encrypted plan/job persistence; and
- Telegram-specific authorization wording and retry parsing.

`LiveGateway` currently owns:

- TDLib loading and version validation;
- TDLib authorization state;
- Telegram catalog loading and progress;
- in-memory chat and sender caches;
- raw Telegram-to-domain mapping;
- global, per-chat, secret-chat, privacy, and history searches;
- capability evaluation; and
- every Telegram mutation.

### Current persistence and search

There is no normalized Retract content database or generic local search index.

- Telegram message/session data is owned by TDLib's encrypted local database.
- Telegram searches execute against TDLib.
- Retract writes a small AES-GCM `jobs.enc` aggregate containing frozen plans and job records.
- The current job store intentionally contains identifiers, reach expectations, counters, titles needed for confirmation, and timestamps, but no message bodies.
- Non-secret Telegram connection settings are stored in `connection-settings.json`.
- The Telegram API hash, TDLib database key, and job-store key are stored in a consolidated macOS Keychain vault. Non-macOS builds currently use protected local key files.
- There is no durable application audit event stream. TDLib logging defaults to verbosity 1 and can be overridden by an environment variable.

This conflicts with the request to reuse an existing persistence/index system for Discord and X: that system does not exist. A new content store is required for archive providers.

### Existing reusable abstractions

The following behavior is genuinely reusable and must be preserved:

- the frontend `RetractApi` adapter seam;
- the backend-owned plan/review/authorize/execute workflow;
- immutable plan IDs and fingerprints;
- confirmation tiers and exact-title confirmation;
- bounded, single-conversation batches;
- execution-time authority rechecks;
- cancellation between provider calls;
- conservative handling of ambiguous outcomes and restarts;
- content-free job persistence;
- targeted refresh of affected conversations;
- normalized content kinds where they are truly cross-provider;
- the Rust sensitive-data detector; and
- the three-pane search, result, and impact-review visual language.

## Current Telegram coupling

Telegram behavior is coupled into every major layer:

| Layer | Coupling |
| --- | --- |
| Domain | Telegram chat kinds, roles, deletion reach, and operations |
| Identifiers | Bare signed `i64` chat, message, and sender IDs |
| Gateway | A single `TelegramGateway` mixes auth, queries, capabilities, and mutation |
| Service | Telegram capability branches, wording, operation ordering, and `FLOOD_WAIT` parsing |
| Persistence | Telegram production/test profiles and plan fingerprints without provider/account identity |
| Settings | TDLib path, API ID/hash, test DC, and Telegram-specific auth stages |
| Search | TDLib global/chat/secret search and Telegram content filters |
| UI | Telegram account setup, chat authority, delete-for-everyone impact, and Telegram operation copy |
| Fixtures | TypeScript reimplementation of planning, capability, search, and privacy rules |

These concepts must not be mechanically renamed. Telegram's `deleteChatHistory`, `deleteMessages`, `leaveChat`, and `deleteChat` represent materially different effects. Hiding them behind a generic `delete` boolean would weaken safety.

## Considered architectures

### Thin provider facade

This approach retains the current domain and adds provider branches around it. It is initially fast, but it spreads provider-name conditionals, cannot represent archive-only evidence honestly, and preserves Telegram IDs as global identity. It is rejected.

### Immediate universal local index

This approach migrates Telegram, Discord, and X into a single normalized database before adding a provider. It offers a uniform query engine, but it rewrites working Telegram search, secret-chat behavior, caching, freshness, and persistence before delivering Discord value. It is rejected for the initial migration.

### Hybrid provider architecture

This is the selected design:

- Telegram remains a live TDLib-backed query source during the initial migration.
- Discord and X archives are imported into a new encrypted normalized store.
- Every query source returns normalized records.
- A provider-neutral query service filters and merges results.
- Provider adapters own native locators, connection flows, archive schemas, capability evaluation, preflight, execution, verification, and error translation.
- Telegram may gain optional local indexing later without changing the provider-facing core.

This is a strangler migration: first protect existing behavior, then introduce boundaries, then move Telegram behind them without rewriting the TDLib implementation.

## Target architecture

```text
React UI
  |- provider/account selector
  |- normalized search and filters
  |- normalized content results
  |- backend-supplied action/impact descriptors
  `- review and job presentation
             |
             v
Tauri IPC contract
             |
             v
Retract application core
  |- AccountService
  |- QueryService
  |- PrivacyAnalysisService
  |- PlanService
  |- AuthorizationService
  |- ExecutionService / JobRunner
  `- ImportService
             |
             v
Provider registry
  |- Telegram provider
  |    |- TDLib connection/auth
  |    |- live catalog/query source
  |    `- Telegram remediation executor
  |- Discord provider
  |    |- versioned Data Package reader
  |    |- local indexed query source
  |    `- manual/external remediation
  |- X provider
  |    |- versioned archive reader
  |    |- local indexed query source
  |    |- manual/external remediation
  |    `- optional official API executor
  `- Future providers
             |
             v
Persistence
  |- existing TDLib encrypted profiles
  |- encrypted normalized archive database + FTS
  |- versioned content-free plan/job state
  `- OS credential vault
```

The core knows provider and account identity because it must route and isolate data. It does not know Telegram, Discord, or X behavior. Looking up a registered adapter by `ProviderKey` is routing, not provider-specific business branching.

## Normalized domain model

### Opaque scoped identities

All provider-facing IDs become opaque strings or UUID-backed application IDs across Rust, IPC, and TypeScript.

**Approved Phase 0 boundary amendment (2026-09-01):** the Telegram characterization stage freezes the current signed numeric wire IDs and requires its fixture values to remain JavaScript-safe. It does not implement, simulate, or prove the future opaque-ID contract. Before the provider-foundation stage changes any production ID type, that stage must first add a failing lifecycle harness containing opaque strings and provider-native values greater than JavaScript's safe-integer range. The harness must cover selection keys, plan construction and fingerprints, job tracking, encrypted persistence/recovery, Rust/TypeScript IPC, and dirty targeted refresh/reconciliation. Only then may production types change to make the harness pass. The Phase 0 numeric-safe fixture may not be cited as satisfying that prerequisite.

```rust
pub struct ProviderKey(String);
pub struct AccountId(Uuid);
pub struct SourceId(Uuid);
pub struct ConversationId(Uuid);
pub struct ContentId(Uuid);
pub struct ActorId(Uuid);
```

No core or frontend code parses provider-native identifiers. Provider-native values remain intact in a locator envelope:

```rust
pub struct ProviderResourceRef {
    pub provider: ProviderKey,
    pub account_id: AccountId,
    pub resource_kind: ResourceKind,
    pub locator_schema: String,
    pub locator_version: u16,
    pub canonical_key: String,
    pub locator_payload: serde_json::Value,
}
```

`locator_payload` is not general application metadata. It is produced, validated, versioned, and consumed only by the owning provider module. Each provider uses a typed Rust locator before serialization:

```text
TelegramMessageLocator { chat_id, message_id }
DiscordMessageLocator { guild_id?, channel_id, message_id }
XPostLocator { post_id, conversation_id? }
```

The canonical key is provider-defined, deterministic, account-scoped, and used for deduplication. Plans fingerprint the provider, account, locator schema/version, and canonical key.

### Account and source

```text
Account
- internal account ID
- provider key
- provider-native account ID
- display name
- username/handle
- avatar reference where available
- connection state
- created/last-seen timestamps

Source
- source ID
- account ID
- live connection or archive import
- source state
- archive fingerprint for archive sources
- schema profile/version
- imported/updated timestamps
- warning summary
```

A single account may eventually have both archive and live sources. An archive never grants live authority.

### Conversation

```text
Conversation
- internal conversation ID
- account/source IDs
- provider-native locator
- normalized kind
- title
- optional parent conversation/community/server
- participant count and normalized participants where available
- archive/live evidence state
- last observed timestamp
```

Normalized kinds are deliberately broad:

```text
direct
group
community_channel
broadcast
public_thread
private_thread
account_feed
other
```

Provider-specific labels such as “Supergroup” remain presentation metadata supplied by the provider, not core branching inputs.

### Content item

```text
ContentItem
- internal content ID
- account/source/conversation IDs
- provider-native locator
- author actor ID
- timestamp and optional edited timestamp
- normalized content kind
- normalized searchable text
- attachment records
- optional reply/thread parent
- external-location availability
- evidence source and observation timestamp
- privacy findings + detector version
- provider metadata schema/version
```

The normalized content taxonomy keeps current useful categories: text, image, video, document, voice, audio, animation, sticker, poll, location, contact, service/system, and other. Provider-specific subtypes remain provider metadata.

### Attachments

Attachment records contain only archive-provided inert metadata needed for search and display: kind, safe display name, size/MIME where present, and a provider-owned locator. Importing an archive does not fetch an external attachment URL.

### Provider metadata

Provider metadata is stored in a versioned envelope but defined as typed structures within provider modules. Core services may retain or round-trip the envelope but may not interpret provider fields.

## Provider contracts

Do not turn `TelegramGateway` into a generic god interface. Split provider responsibilities into focused ports.

```rust
#[async_trait]
pub trait ProviderRegistration: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;
    fn query_source(&self, source: &SourceRecord) -> Arc<dyn QuerySource>;
    fn remediation(&self, account: &AccountRecord) -> Option<Arc<dyn RemediationProvider>>;
    fn live_connection(&self) -> Option<Arc<dyn LiveConnection>>;
    fn archive_importer(&self) -> Option<Arc<dyn ArchiveImporter>>;
}

#[async_trait]
pub trait QuerySource: Send + Sync {
    async fn list_conversations(&self, request: ConversationQuery)
        -> Result<Page<ConversationRecord>, ProviderError>;
    async fn search(&self, request: ContentQuery)
        -> Result<Page<ContentRecord>, ProviderError>;
    async fn resolve(&self, refs: &[ProviderResourceRef])
        -> Result<Vec<ContentRecord>, ProviderError>;
}

#[async_trait]
pub trait ArchiveImporter: Send + Sync {
    async fn inspect(&self, source: ArchiveSource)
        -> Result<ImportInspection, ImportError>;
    async fn import(&self, source: ArchiveSource, sink: &dyn ImportSink)
        -> Result<ImportSummary, ImportError>;
}

#[async_trait]
pub trait RemediationProvider: Send + Sync {
    async fn actions_for(&self, targets: &[ProviderResourceRef])
        -> Result<Vec<ActionDescriptor>, ProviderError>;
    async fn preflight(&self, plan: &RemediationPlan)
        -> Result<PreflightResult, ProviderError>;
    async fn execute_batch(&self, batch: &ExecutionBatch)
        -> Result<BatchResult, ProviderError>;
    async fn verify(&self, batch: &ExecutionBatch)
        -> Result<VerificationResult, ProviderError>;
}
```

Telegram auth remains typed inside the Telegram provider. Archive providers use a generic file-import flow. X OAuth uses an X provider connection flow. The shell accesses these through provider/account state rather than one application-wide Telegram `AuthStage`.

## Capability and remediation model

Static provider capabilities describe supported integration modes:

```text
live_connection
archive_import
conversation_listing
content_search
media_metadata
external_location
automatic_remediation
bulk_remediation
verification
```

Static capabilities never authorize a particular item. Each selected item receives backend-derived action descriptors:

```text
ActionDescriptor
- stable action ID
- normalized action kind
- expected effect
- availability
- availability reason code and user-safe explanation
- live-preflight requirement
- batch constraints
- confirmation tier
- destructive/irreversible flag
- optional cost/rate advisory
```

Normalized action kinds:

```text
delete_remote_item
remove_for_current_account
clear_conversation
leave_conversation
delete_conversation
delete_by_actor
open_externally
manual_remediation
remove_local_import
```

Expected effects remain explicit:

```text
removed_for_all_participants
removed_for_current_account_only
public_content_removed
membership_removed
container_destroyed
local_import_removed
manual_or_unknown
```

Availability is one of:

```text
executable
live_preflight_required
manual_only
unavailable(reason)
```

The UI renders these descriptors. It does not infer actions from provider names, conversation roles, or native permissions. Provider adapters may use native permissions internally to create the descriptors.

One executable plan is always scoped to exactly one provider and one account. A cross-provider selection is partitioned into separate plans and separate authorization/execution units. Manual actions may be summarized together but never merged into an executable destructive plan.

## Query and privacy-analysis flow

The query service accepts provider/account/source/conversation filters expressed over normalized fields. It routes each selected source to its registered `QuerySource`, applies shared normalized filtering where needed, and merges pages using timestamp plus stable content ID.

During the initial migration:

- Telegram queries continue to execute through TDLib and return normalized transient records.
- Discord and X queries execute against the encrypted local FTS index.
- All-source queries carry a composite cursor with one opaque cursor per query source.
- Query results display provider, account, source freshness, conversation context, and remediation availability.
- Stale search responses remain discardable through the current frontend generation/request logic.

Privacy analysis consumes normalized searchable text, content kind, and safe attachment metadata. The existing Rust detector remains the single implementation. Archive imports may compute findings during ingestion and record a detector version; a later detector version can rescan local records without reimporting. Live Telegram privacy search continues to page TDLib data without persisting bodies in the new database.

## Persistence design

### Encrypted normalized database

Use SQLite with FTS5 through a SQLCipher-backed build. The database is encrypted at rest with a key separate from the job-store and provider credential keys. The dependency must be source-pinned and built through the existing containerized native-dependency workflow. A failed cross-platform SQLCipher build must stop the persistence task for a design amendment; plaintext content storage is not an acceptable fallback.

Initial schema responsibilities:

```text
schema_migrations
accounts
sources
conversations
actors
content_items
attachments
import_runs
import_warnings
content_fts
plans
jobs
job_items
job_events
```

Design rules:

- Internal primary keys are UUID strings.
- Provider-native values are opaque text inside versioned locator envelopes.
- `(provider_key, account_id, resource_kind, canonical_key)` is unique.
- Imports use bounded transactions and upsert by stable canonical key.
- Content and FTS rows are stored only for archive/local-index sources.
- Raw archive files are never copied into the application data directory by default.
- Capability observations may be cached with timestamps but are never execution authority.
- Job events contain status codes, counts, timestamps, provider/account IDs, and opaque target references only; no content bodies.
- Removing an imported source deletes dependent normalized records, checkpoints the WAL, enables SQLite secure-delete behavior, and compacts the database. Documentation must state that filesystem snapshots, SSD behavior, and backups can retain old encrypted blocks.

### Credential storage

Replace fixed Telegram vault fields with a versioned namespaced secret map:

```text
telegram/<account-or-profile>/api-hash
telegram/<account-or-profile>/tdlib-database-key
retract/job-store-key
retract/content-index-key
x/<account>/oauth-access-token
x/<account>/oauth-refresh-token   # only when explicitly enabled
```

macOS continues using Keychain. Other platforms require an OS credential-store implementation before provider tokens are supported there. A local plaintext or merely permission-protected X token file is not acceptable.

### Existing state migration

The current `SecureJobStore` has a narrower preservation contract than the future migration described here: it writes and syncs an encrypted temporary file before replacing `jobs.enc`, so a characterized temporary-write failure leaves the last authenticated file unchanged. A successful replacement retains no application-managed backup or general rollback copy. The migration requirements below are therefore prospective; provider-foundation/persistence work must not assume a rollback facility already exists.

- Existing TDLib directories are not moved, rewritten, deduplicated, or treated as the normalized index.
- Existing Telegram connection settings remain readable and are upgraded through an atomic, versioned migration.
- `RTRCT01` and `RTRCT02` job-store fixtures remain readable.
- Terminal legacy jobs are imported as Telegram-profile historical jobs without content.
- Nonterminal legacy jobs are retained but stopped with `migration_requires_new_review`; they are never resumed under an unverified account identity.
- New plans include provider, account, source, locator schema/version, canonical resource keys, and expected actions in their fingerprints.
- New authenticated-encryption associated data includes store schema, provider, and account scope where the store is account-specific.
- A failed or interrupted migration keeps the original file and fails closed. Migration writes a new file/database, verifies it, and atomically switches only after success.

## Discord integration

### Supported first milestone

Discord Phase 1 is local Data Package import, search, privacy analysis, and assisted/manual remediation.

The importer:

1. Opens the selected ZIP in Rust without extracting it wholesale.
2. Validates paths, entry types, declared sizes, observed sizes, compression ratios, and configured global limits.
3. Detects a supported Discord archive schema profile.
4. Streams channel metadata and sent-message records.
5. Preserves message, channel, and guild IDs when present.
6. Normalizes DM, group-DM, server, channel, thread, timestamp, text, and attachment metadata.
7. Writes bounded upsert batches into the encrypted store.
8. Records import fingerprint, account identity, counts, schema profile, and content-free warnings.
9. Exposes imported content through the same query and privacy interfaces.

Archive records carry `archive_snapshot` evidence and never imply current membership, message existence, or permission.

### Remediation posture

Retract must not accept Discord user tokens or automate a normal user account. A successful archive import exposes `manual_remediation` and, when a safe locator is available, `open_externally`; it does not expose executable user-account deletion.

An optional later Discord bot moderation integration is a separate actor and product mode. It can operate only in guild channels where the bot is installed and currently has the documented permissions. It cannot represent the user's DM history and is not part of the first Discord milestone.

The exact current Data Package filenames and wrappers are treated as a schema-profile concern. Implementation begins only after sanitized synthetic fixtures reproduce a current official export structure. Real private exports are never committed.

## X integration

### Archive milestone

X Phase 1 imports the official archive locally. It prioritizes posts, replies, quote/repost relationships, media references, and thread/conversation context. DMs may be added through another schema profile after post ingestion is stable.

The X archive reader owns format detection, inert JavaScript-wrapper stripping where fixture-validated, partition discovery, schema validation, and typed provider locators. Archive HTML and JavaScript are never loaded into the webview or executed.

Archive-only items expose search, privacy analysis, external location where possible, and manual remediation.

### Optional official API deletion

X API deletion is an optional remediation capability, never an ingestion requirement.

- Use OAuth 2.0 Authorization Code with PKCE for the native application.
- Request only `tweet.read`, `tweet.write`, and `users.read` initially.
- Do not request `offline.access` unless the user separately enables persistent connection.
- Store access and optional refresh tokens only in the credential vault.
- Bind plans to the authenticated native account ID and immutable post IDs.
- Preflight ownership/existence when supported and economically acceptable.
- Show rate and cost implications before execution.
- Execute sequentially or in provider-approved bounded batches.
- Honor structured rate-limit reset data.
- Stop at the user-configured spend cap or exhausted credits.
- Treat only an explicit successful deletion response as confirmed.
- Treat lost responses and transport ambiguity as ambiguous, not success and not automatically safe to replay.

X DM deletion is not implied by post deletion support and remains unavailable unless an official capability is separately verified.

## Import isolation and archive security

Archive parsing lives in a Rust crate/module that has no network client and no credential access. It receives a file handle and writes normalized records through a bounded `ImportSink`.

It must reject:

- absolute paths;
- `..` traversal;
- backslash/platform path ambiguity;
- NULs and invalid normalized names;
- duplicate normalized entry names;
- symlinks, hard links, device files, and other special entries;
- excessive entry count;
- excessive per-entry or total uncompressed bytes;
- extreme compression ratios;
- excessive nesting or JSON depth;
- oversized JSON strings or records;
- invalid UTF-8 where the schema requires UTF-8;
- malformed, truncated, or unsupported wrappers; and
- archive structures that cannot be assigned to a supported schema profile.

All limits are named constants with deterministic error codes and tests. Imports are cancellable, report progress by entries/records/bytes, and roll back an incomplete batch safely. Filenames, display names, message text, and URLs are hostile input and are escaped before display. URLs are never fetched during import.

## Safety invariants retained and generalized

- Only backend-created frozen plans execute.
- Plan fingerprints bind provider, account, source, action, expected effect, native locator schema/version, canonical keys, confirmation data, and immutable targets.
- Every destructive plan requires a single-use device-owner grant bound to that fingerprint.
- Executable plans contain exactly one provider and one account.
- Execution performs provider-specific live preflight immediately before action.
- A provider may narrow a plan by skipping unavailable targets, but may never expand it.
- A blocked or failed action may not be substituted with a different effect.
- Batches remain provider-bounded and resource-container bounded where the provider requires it.
- Cancellation prevents later calls; in-flight calls may still complete.
- Only demonstrably idempotent frozen-target jobs may resume automatically.
- Broad, dynamic, cost-bearing, or ambiguous jobs require new review after an uncertain restart.
- Permanent container destruction remains a separate critical action.
- Cleanup-before-leave ordering remains part of Telegram's provider-specific plan preparation.
- User-visible results distinguish confirmed, skipped, failed, already unavailable, manual, and ambiguous outcomes.

## UI evolution

The three-pane layout and current visual language remain.

Minimum additions:

- a provider/account/source selector near the existing account area;
- connected-live, archive-imported, disconnected, and configuration-required states;
- provider and source-freshness badges on mixed-source results;
- provider-neutral filters plus provider-specific filter descriptors where necessary;
- impact rows supplied by backend action descriptors;
- import progress and source-removal controls; and
- explicit manual-remediation and external-location actions.

Provider iconography is registered as presentation metadata in one UI registry. Core selection, search, planning, and execution code does not inspect provider logos or names.

Telegram-specific setup and auth remain typed provider UI components behind the registry. Archive import uses a reusable source-import component. X OAuth uses an X-specific connection component. A fully schema-generated authentication UI is intentionally not required.

## Structured errors and logging

Providers translate native failures into a shared error model:

```text
authentication_required
permission_changed
not_found
already_removed
rate_limited(retry_at)
cost_limit_reached
transient
permanent
ambiguous_outcome
unsupported_schema
invalid_archive
```

Raw provider response strings are retained only in redacted developer diagnostics when safe; they are not copied into durable jobs or normal UI errors.

Introduce a content-free event schema for import and remediation lifecycle events. Events contain provider/account/source IDs, opaque resource hashes or keys required for recovery, status codes, counts, and timestamps. They never contain message bodies, credentials, auth codes, archive paths, or unnecessary attachment names.

## Performance model

- Archive ZIP entries and records are streamed.
- Database writes use bounded transactions.
- FTS and relational filters run in SQLite rather than loading all records.
- Results are paginated with stable cursors.
- All-source queries use a composite per-source cursor and a stable timestamp/content-ID merge.
- Imports and long-running remediation are cancellable and report progress.
- The UI may block initial use of a newly selected source until its required catalog/import state is ready, but it must display measurable progress.
- The result list should be virtualized before million-record archive support is declared.
- Provider concurrency and batch sizes are provider-defined and bounded.

## Security risks and mitigations

| Risk | Required mitigation |
| --- | --- |
| Cross-provider/account plan replay | Scope every ID, lookup, fingerprint, grant, plan, and job by provider and account |
| Local index theft | SQLCipher encryption, separate Keychain key, no plaintext fallback |
| ZIP traversal/bombs | Streaming parser, strict normalized paths, entry/size/ratio/depth limits |
| Imported active content | Never execute/render archive HTML or JS; escape all strings |
| Hostile URLs | Never fetch on import; open only through explicit user action and validated schemes |
| Stale archive authority | Archive evidence can only create manual or live-preflight-required actions |
| Credential leakage | Namespaced OS vault, zeroization, no token values in IPC/logs |
| Provider error leakage | Structured errors and redacted diagnostics |
| Permission changes | Live preflight immediately before mutation and after waits |
| Ambiguous API outcome | Explicit ambiguous state; no unsafe automatic retry |
| Cost/rate exhaustion | Provider-aware queue, reset timestamps, user spend cap |
| Scope broadening | Frozen target list and action effect; property tests prevent substitution/expansion |
| Prompt spoofing | Continue sanitizing control/direction characters and show provider/account/native target token |
| SSD/backups retain deleted local blocks | Encrypted database, secure-delete/compaction, honest documentation |

## Migration risks and controls

### Opaque ID migration

Changing `i64`/JavaScript-number identifiers affects domain structs, IPC DTOs, selection keys, refresh reconciliation, jobs, fixtures, and UI props. Phase 0 deliberately characterizes only the existing JavaScript-safe numeric Telegram contract. The first provider-foundation ID change must begin with the test-first opaque/string and greater-than-safe-integer lifecycle harness specified above, spanning selection, planning, jobs, persistence, IPC, and targeted refresh; no production ID type may change before that RED evidence exists. During migration, Telegram adapters parse native numeric strings internally; the frontend never does.

### Plan/job compatibility

Existing plans omit provider/account identity. Retain readers, preserve terminal history, and stop nonterminal legacy work for fresh review. Do not synthesize authority from a profile name alone.

### Telegram behavior drift

Do not replace TDLib search or its database during Telegram migration. Wrap and normalize the current behavior. Test raw TDLib mapping, secret chats, catalogs, pagination, capabilities, deletion ordering, retry, cancellation, and targeted refresh before and after.

### Fixture drift

The TypeScript fixture currently reimplements business rules. Move toward serialized backend fixture snapshots or a provider-contract fixture so the demo remains presentation data rather than an alternate policy engine.

### Cross-platform credentials

macOS Keychain is the current strong path. X live tokens may ship only on platforms with an approved OS credential-store implementation. Archive-only support can remain cross-platform when its encrypted database key is protected equivalently.

## Regression tests required before refactoring Telegram

### Backend characterization

- TDLib authorization-state transitions and secret handling.
- Main/archive catalog enumeration, deduplication, progress, caching, dirty updates, and targeted refresh.
- Raw TDLib chat mapping for direct, secret, basic group, supergroup, channel, owner, full admin, limited admin, restricted member, and departed member.
- Raw message mapping for every current content kind, captions, albums, locations, contacts, service messages, pinned state, and delete capability.
- Global, scoped, empty-query, secret-chat, privacy, and sender searches with pagination and deduplication.
- Every current filter and date/direction boundary.
- Selected-message, own-history, clear-history, leave, admin-wide, sender-wide, self-removal, and group-destruction plan construction.
- Capability changes between review and execution.
- Per-message rechecks before deletion and after rate-limit waits.
- Bounded batch execution, partial results, cancellation, retry, restart, and ambiguous broad-operation behavior.
- Native prompt sanitization and exact plan-bound system grants.
- `RTRCT01`/`RTRCT02` load, tamper, cross-profile, current pre-replacement preservation, and restart fixtures; the future migration adds the separate rollback fixture required for its write/verify/switch path.
- Proof that no message body enters persisted plan/job state or logs.

### Contract and frontend characterization

- Rust JSON fixtures conform to TypeScript request/response shapes.
- Provider-foundation prerequisite, explicitly not satisfied by Phase 0: opaque strings and greater-than-JavaScript-safe-integer provider values survive selection, planning, job tracking, encrypted persistence, IPC, and targeted refresh.
- Provider/backend action descriptors exclusively control visibility, effect copy, confirmation, and disabled reasons.
- Search generation rejects stale responses.
- Provider/account/source changes clear incompatible selections and never retarget a plan.
- Album atomicity, hidden selections, and cross-conversation review remain correct.
- No execution occurs before review, irreversible acknowledgement, and authorization.
- Completed chat removal uses targeted reconciliation without a global reload.
- Every job terminal and retry state remains visible and truthful.

### Live release gate

Continue using Telegram test-DC disposable identities and the destructive matrix in `docs/TEST_PLAN.md`. The acceptance criterion is user-visible and API-level parity before and after migration.

## Provider contract tests

Every provider implementation must satisfy shared tests for:

- stable provider/account-scoped identities;
- deterministic canonical keys and import deduplication;
- normalized content and conversation records;
- capabilities that never overstate authority;
- provider-native locator round trips;
- structured, content-free errors;
- cancellation and progress;
- action scope that cannot expand after review; and
- source deletion that removes only the selected local source.

Archive providers additionally satisfy traversal, special-entry, compression-bomb, malformed/truncated JSON, invalid wrapper, UTF-8, deep nesting, schema-drift, oversized-record, and hostile-URL tests using synthetic fixtures only.

## Staged delivery

Each stage is a separate specification/implementation plan and should land through reviewable GitHub issues and pull requests.

1. **Telegram characterization suite** — protect the existing integration, domain, persistence, and UI behavior.
2. **Provider foundation** — first land the RED opaque/string and greater-than-safe-integer lifecycle harness, then introduce scoped opaque IDs, provider registry, normalized records, action descriptors, structured errors, and compatibility API.
3. **Encrypted archive persistence** — SQLCipher/FTS store, migrations, import lifecycle, source deletion, and security limits.
4. **Telegram provider migration** — move current TDLib behavior behind focused query/connection/remediation ports without changing results.
5. **Discord archive import** — streaming parser, schema profiles, synthetic fixtures, normalized ingestion, and progress.
6. **Discord provider UX** — account/source presentation, hierarchy/context, search/privacy integration, and manual/external remediation.
7. **X archive import** — versioned post/reply/media reader and normalized ingestion.
8. **X manual and optional API remediation** — external location first; OAuth PKCE and cost/rate-aware deletion as a separately reviewable capability.
9. **Cross-provider search** — composite pagination, provider/account filters, mixed-source display, and plan partitioning.
10. **Privacy scan and documentation generalization** — uniform rescans, capability matrix, threat model, test plan, README, and release review.

No single PR should combine the provider foundation, Telegram migration, and an archive provider.

## Likely file changes

### Existing core/backend files

- `crates/cleaner-domain/src/lib.rs`
- `crates/cleaner-domain/src/sensitive.rs`
- `src-tauri/src/gateway.rs`
- `src-tauri/src/service.rs`
- `src-tauri/src/model.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/live_gateway.rs`
- `src-tauri/src/tdjson.rs`
- `src-tauri/src/secure_store.rs`
- `src-tauri/src/connection_settings.rs`
- `src-tauri/src/error.rs`
- `src-tauri/Cargo.toml`

### Existing frontend files

- `src/types.ts`
- `src/api-contract.ts`
- `src/api.desktop.ts`
- `src/api.fixture.ts`
- `src/App.tsx`
- `src/components/Sidebar.tsx`
- `src/components/SearchToolbar.tsx`
- `src/components/ResultsList.tsx`
- `src/components/ImpactPanel.tsx`
- `src/components/ConfirmDialog.tsx`
- `src/components/AuthGate.tsx`
- `src/components/ConnectionSettingsDialog.tsx`
- `src/components/common.tsx`
- `src/demo.ts`

### Likely new modules

```text
crates/retract-domain/
crates/archive-import/
src-tauri/src/providers/
src-tauri/src/providers/telegram/
src-tauri/src/providers/discord/
src-tauri/src/providers/x/
src-tauri/src/persistence/
src-tauri/src/query/
src-tauri/src/remediation/
src/providers/
```

Crate boundaries are reserved for provider-neutral domain logic and the network-isolated archive parser. Other modules should remain within `src-tauri` until their independence is proven.

### Documentation and CI

- `README.md`
- `PLAN.md`
- `docs/THREAT_MODEL.md`
- `docs/TEST_PLAN.md`
- `docs/LOCAL_LIVE_TEST.md`
- `.github/workflows/secure-build.yml`
- `.github/workflows/release.yml`
- `.github/ISSUE_TEMPLATE/*`

## Git and issue workflow

The repository uses GitHub, conventional issue templates, a pull-request template, Dependabot, secure-build CI, and release CI. No implementation issues are created during Phase 0. After this design is approved, create one issue per staged delivery item, with explicit dependencies and acceptance tests, and use one or more focused PRs per issue.

## Completion criteria for the architecture migration

The provider foundation and Telegram migration are complete only when:

- Telegram's existing test suite and live test-DC matrix pass unchanged from the user's perspective;
- no core execution or UI branch selects behavior by comparing a provider name;
- every content/plan/job identity is provider/account scoped;
- backend-derived action descriptors drive the impact and review UI;
- existing TDLib sessions and settings still load;
- legacy persisted jobs remain visible and cannot replay unsafely;
- no Telegram message body is copied into the normalized archive database by default; and
- a synthetic future archive provider can ingest normalized records, search them, produce manual remediation actions, and delete its local source without changing the search/privacy engine.

Discord support is complete only when a synthetic official-format archive can be safely streamed, deduplicated, searched, privacy-scanned, removed locally, and presented with honest manual/external remediation capabilities—without any Discord user-token automation.

X archive support is complete only when it provides the same local guarantees. Optional X deletion is complete only when official authentication, ownership scoping, cost/rate controls, immutable review, result verification, cancellation, and ambiguous-outcome handling pass with a disposable account.

## Official provider references

- [Discord Data Package](https://support.discord.com/hc/en-us/articles/360004957991-Your-Discord-Data-Package)
- [Discord automated user account policy](https://support.discord.com/hc/en-us/articles/115002192352-Automated-User-Accounts-Self-Bots)
- [Discord OAuth2 scopes](https://docs.discord.com/developers/platform/oauth2-and-permissions)
- [Discord message API](https://docs.discord.com/developers/resources/message)
- [Discord rate limits](https://docs.discord.com/developers/topics/rate-limits)
- [X account archive](https://help.x.com/en/managing-your-account/accessing-your-x-data)
- [X delete-post endpoint](https://docs.x.com/x-api/posts/delete-post)
- [X OAuth 2.0 PKCE](https://docs.x.com/fundamentals/authentication/oauth-2-0/authorization-code)
- [X authentication mapping](https://docs.x.com/fundamentals/authentication/guides/v2-authentication-mapping)
- [X API rate limits](https://docs.x.com/x-api/fundamentals/rate-limits)
- [X API usage and billing](https://docs.x.com/x-api/fundamentals/post-cap)
