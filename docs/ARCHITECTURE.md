# Architecture

Retract is a local-first Tauri 2 application: React and TypeScript render the UI, while Rust owns credentials, provider access, immutable cleanup plans and execution. There is no hosted Retract service. Telegram uses pinned, statically linked TDLib; Discord uses a locally imported Data Package and, only for cleanup, a verified matching user session. Other providers are not enabled.

## Component boundaries

```text
React UI → scoped v2 IPC → application runtime / provider registry
  ├─ TelegramConnection → TelegramProvider
  │    ├─ TelegramQuery → TelegramRead
  │    └─ TelegramCleanup → read + mutation ports + FoundationStore
  │         └─ shared native LiveGateway → static TDLib → Telegram
  └─ Discord application
       ├─ bounded ZIP reader → normalizer → encrypted archive worker
       ├─ archive-backed query / privacy scan
       └─ reviewed cleanup → verified session → bounded Discord HTTP
```

| Location | Responsibility |
| --- | --- |
| [`src/`](../src/) | UI, closed DTO decoders, desktop adapter and context-scoped state |
| [`crates/retract-domain/`](../crates/retract-domain/) | Provider-neutral identities, records, actions, plans and lifecycle contracts |
| [`crates/cleaner-domain/`](../crates/cleaner-domain/) | Telegram planning rules and shared heuristic sensitive-data detection |
| [`crates/discord-archive/`](../crates/discord-archive/) | Bounded ZIP inventory, schema inspection and streaming typed records; no network, credentials or database ownership |
| [`src-tauri/src/providers/`](../src-tauri/src/providers/) | Provider registration, query ports and reviewed lifecycle enforcement |
| [`src-tauri/src/providers/telegram/`](../src-tauri/src/providers/telegram/) | Connection, normalization, native ports and one reviewed cleanup owner |
| [`src-tauri/src/providers/discord/`](../src-tauri/src/providers/discord/) | Import coordination, archive queries, account-bound sessions and exact-message cleanup |
| [`src-tauri/src/persistence/`](../src-tauri/src/persistence/) | Authenticated job state, profile locks and bounded SQLCipher archive storage |

The parser is a library running in a tracked blocking task, not a sandboxed subprocess. Docker isolates portable build/test commands, not the installed application's runtime.

## Identity and IPC

Provider-native IDs remain typed strings inside provider locators. The UI receives opaque resource references and explicit provider/account/source scope, not JavaScript numeric IDs. Session generation is ephemeral; account and source mappings are durable. Display names, titles, roles and archive metadata do not grant authority.

Telegram publishes usable context only after authenticated `getMe` verification and durable identity mapping. Discord archive search needs no live token; cleanup requires `/users/@me` to match the archive owner exactly. Source activation constructs and bootstraps a candidate before retiring the previous workspace, so a failed open leaves the old workspace usable.

The backend checks contract version, scope, generation, resource references and closed payload schemas. Stale search, polling, settings and targeted-refresh results cannot replace another context. A dirty conversation refresh resolves only matching-scope conversations instead of reloading the entire catalog. Read-only startup reconciliation can rediscover an advancing context with bounded retries; it never repeats a save, credential submission or destructive action.

## Reviewed cleanup

1. Resolve selected references and current capabilities. The backend freezes exact targets, provider recipe, ordered effects, confirmation requirements and restart policy in one fingerprinted plan.
2. Present that plan for review. A fresh native device-owner prompt identifies the immutable target and plan token; its grant binds the fingerprint, scope and generation, expires after 60 seconds and is single-use.
3. Recheck context and provider state. Persist the job before mutation, and persist accepted progress before later batches or membership changes. Required save failures block further mutation.
4. Execute bounded work with cancellation and rate-limit checks. Report selected, eligible, deleted, skipped, failed and uncertain outcomes separately. An ambiguous response is never confirmed deletion.

Raw remediation is not a production escape hatch. The webview cannot mint plans or grants, widen a target list, reuse authorization or start the same plan twice. Recovered destructive jobs never auto-replay after a process restart; continuing requires a newly reviewed plan and fresh owner authorization. Supported historical seals remain readable without being rewritten under a newer policy; see [storage and compatibility](STORAGE.md#job-state-and-migration).

### Telegram

`TelegramQuery` owns only `TelegramRead` and `EngineContext`, with no direct mutation, cleanup-state or store API. Its production read port shares the native `LiveGateway`, which retains identity persistence and can indirectly prolong the profile-lock lifetime. `TelegramCleanup` is the single owner of plans, jobs, grants, cancellation, transition serialization, persistence quarantine and accepted workers. Native DTOs and TDLib calls stay behind the Telegram provider boundary.

Selected-message cleanup rechecks each frozen ID immediately before deletion and after waits, with at most 100 IDs from one chat per batch. Everyone-scoped cleanup never falls back to self-only deletion. Local history removal is a separate reviewed effect. Cleanup precedes leaving: use whole-history revoke where available, otherwise eligible all-participant messages for an authorized admin or eligible outgoing messages for a member. Permanently dissolving an owned group remains a separate critical action.

Catalog/search preserve normal and secret-chat semantics, pinned protection, media/album handling and hidden selections. Secret-chat availability is limited to history known by the local session. Privacy scans follow valid continuation cursors even when TDLib omits converted messages from a page; approximate totals and unsupported link previews do not define completeness.

### Discord

The import coordinator retains the selected regular-file descriptor, validates identity and hashes, and streams bounded records into one encrypted worker. Incomplete imports remain hidden; explicit retry uses durable checkpoints and rotates mutation authority. ZIP/schema details and quotas are in [storage](STORAGE.md#discord-data-package-profile).

Release builds accept browser-neutral, one-way manual token submission; memory-only sessions are the default and Keychain persistence is opt-in. The backend never returns a saved token to the webview. Browser-debug capture is development-only synthetic compatibility code, not a supported release sign-in method.

Cleanup freezes exact owner-authored archive channel/message IDs. Before each deletion, live ownership, timestamp, text and attachment identity must match the frozen evidence; live responses cannot discover or add targets. Requests serialize per channel, use bounded cross-channel concurrency/account pacing and honor bounded server delays. Success or exact absence completes a target; permission/authentication failures are explicit, and exhausted network ambiguity remains uncertain. Remote deletion does not remove the local archive or original ZIP.

## Compatibility and verification

Keep frozen synthetic fixtures unchanged: [`telegram-ipc-contract.json`](../src/test/fixtures/telegram-ipc-contract.json) covers historical v1 numeric DTOs, not current opaque-ID IPC. [`telegram-migration/`](../src-tauri/test-fixtures/telegram-migration/) pins all nine native recipes and authenticated v3 ciphertext from the pre-extraction implementation. [`discord-import/`](../src-tauri/test-fixtures/discord-import/) pins the accepted export profile; encrypted storage fixtures test historical readers. These are synthetic compatibility evidence, not production secrets or authority to execute old jobs.

The provider-boundary check rejects query-to-mutation coupling and parser/network/storage ownership violations. The full [test plan](TEST_PLAN.md) covers scoped lifecycle, recovery, malformed inputs, production fixture exclusion and manual provider/native cases. Store release-specific outcomes in PRs and releases with source/CI links, not in permanent development diaries. See the [threat model](THREAT_MODEL.md) for trust assumptions and residual risks.
