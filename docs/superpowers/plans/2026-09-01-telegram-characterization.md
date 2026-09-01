# Telegram Characterization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish an executable regression contract proving Retract's current Telegram wire format, TDLib normalization, search, cleanup safety, persistence, authentication, and UI behavior before introducing provider-neutral types.

**Architecture:** This plan changes tests, synthetic fixtures, and test-only helpers only. It does not rename Telegram types, change production behavior, add provider abstractions, or introduce content persistence. Tests lock the current behavior at the domain, TDLib adapter, service, encrypted-store, IPC, and React boundaries so the later provider refactor can prove parity.

**Tech Stack:** Rust 2024, Tauri 2, TDLib JSON fixtures, serde/serde_json, React 19, TypeScript 6, Vitest 4, Testing Library, Docker BuildKit

**Spec:** `docs/superpowers/specs/2026-09-01-multi-provider-architecture-design.md`

## Global Constraints

- Retract remains free, open source, local-first, and usable without hosted infrastructure.
- Existing Telegram behavior, TDLib profiles, settings, sessions, plans, and terminal job history must survive future migration.
- This plan adds no production provider abstraction and makes no user-visible behavior change.
- The Rust backend remains the destructive-action trust boundary.
- Everyone-scoped deletion must never fall back to self-only deletion.
- Message bodies, sensitive attachment names, API credentials, auth codes, and tokens must not enter persisted jobs or diagnostic logs.
- All fixtures must be synthetic and must not contain a real account, session, database, archive, phone number, API credential, or private conversation.
- No production bundle may include the screenshot fixture adapter or synthetic Telegram catalog.
- Every test addition follows red-green verification: demonstrate the missing assertion/failure, add the smallest test support required, then prove the focused and full suites pass.
- Use `apply_patch` for repository edits and preserve unrelated user changes.

## Plan boundary and follow-on sequence

This is the first executable plan in the approved multi-provider roadmap. It intentionally stops before production refactoring. Subsequent plans are written after this gate lands, in this order:

1. provider foundation;
2. encrypted archive persistence;
3. Telegram provider migration;
4. Discord archive import;
5. Discord provider UX/remediation;
6. X archive import;
7. X manual and optional official-API remediation;
8. cross-provider search; and
9. privacy/documentation generalization.

The later plans must consume the actual contracts and test helpers produced here rather than guessing their final signatures in advance.

### Approved Phase 0 ID boundary amendment

Phase 0 freezes Telegram's current signed numeric IPC IDs, and its fixture deliberately keeps them inside JavaScript's safe-integer range. That assertion proves compatibility only for the present Telegram wire contract; it is not an opaque-ID or large-ID test.

- [ ] **Provider-foundation prerequisite:** before changing any production identifier type, add a behavior-level RED lifecycle harness using opaque strings and provider-native values greater than JavaScript's safe-integer range. It must cover selection keys, planning/fingerprints, job tracking, encrypted persistence and recovery, Rust/TypeScript IPC, and dirty targeted refresh/reconciliation. Make the production ID migration pass that harness in the same provider-foundation change. Do not cite the Phase 0 numeric-safe fixture as sufficient.

### Existing encrypted-store preservation boundary

The current store writes and syncs a new encrypted temporary file before replacing `jobs.enc`; the characterization suite proves that a temporary-write failure preserves the prior authenticated file. After a successful replacement, the application keeps no backup or general rollback copy. A later store migration must write and verify a separate new store, retain the original until verification succeeds, and switch atomically only after success.

---

### Task 1: Record the Telegram regression gate and prove the starting baseline

**Files:**
- Create: `docs/TELEGRAM_CHARACTERIZATION.md`
- Read: `docs/TEST_PLAN.md`
- Read: `docs/THREAT_MODEL.md`
- Read: `package.json`

**Interfaces:**
- Consumes: the existing automated commands and destructive test-DC matrix.
- Produces: `docs/TELEGRAM_CHARACTERIZATION.md`, the acceptance checklist every later provider-foundation and Telegram-migration PR must cite.

- [ ] **Step 1: Run the existing frontend suite before changing tests**

Run:

```bash
npm test
```

Expected: Vitest exits 0 with the existing `src/App.test.tsx`, `src/api.test.ts`, and connection-settings tests passing. If it does not, stop and record the pre-existing failure instead of weakening an assertion.

- [ ] **Step 2: Run the existing domain suite before changing tests**

Run:

```bash
cargo test --manifest-path crates/cleaner-domain/Cargo.toml
```

Expected: exit 0; deletion-plan and sensitive-data tests pass.

- [ ] **Step 3: Run the existing Tauri backend suite before changing tests**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: exit 0; TDLib loading, secure-store, gateway, and service tests pass.

- [ ] **Step 4: Create the characterization gate document**

Create `docs/TELEGRAM_CHARACTERIZATION.md` with this structure and language:

```markdown
# Telegram behavior contract

This document is the regression gate for moving Telegram behind Retract's provider architecture. The migration passes only when the automated contract below and the applicable disposable test-DC matrix in `docs/TEST_PLAN.md` produce the same observable behavior before and after the refactor.

## Automated contract

- IPC response and request field names remain compatible until an explicitly versioned migration changes both Rust and TypeScript.
- TDLib chat, membership, message, media, pin, album, sender, and delete-property values normalize deterministically.
- Main/archive catalogs deduplicate chats and targeted refresh never requires a global catalog reload.
- Global, scoped, empty-query, secret-chat, privacy, date, direction, pin, conversation-kind, and content-kind searches retain their current semantics.
- Plans freeze only the reviewed IDs and expected reach.
- Every frozen message is rechecked immediately before deletion and after a rate-limit wait.
- Cleanup happens before leave; permanent group deletion remains separate.
- Everyone-scoped work never becomes self-only work.
- Broad ambiguous jobs stop after restart; safe frozen-ID jobs retain bounded cursor behavior.
- Persisted job state remains encrypted, profile-bound, tamper-evident, and content-free.
- Telegram authentication secrets remain absent from settings JSON and frontend snapshots.
- The UI preserves album atomicity, hidden selections, review-before-execute, progress, cancellation, and targeted reconciliation.

## Live test-DC contract

Use disposable identities and execute the search/catalog, destructive, confirmation/abuse, reliability, and accessibility sections of `docs/TEST_PLAN.md`. Store no account credentials, chat exports, message contents, or TDLib databases in the repository. Record only commit IDs, environment versions, role/capability labels, operation names, provider result codes, counts, and pass/fail outcomes.

## Refactor parity rule

Run this gate on the last pre-refactor commit and on the Telegram-provider migration commit. A changed result requires either a defect fix approved as a separate behavior change or a revision to the architecture design; it may not be dismissed as an internal refactor difference.
```

- [ ] **Step 5: Check the documentation patch**

Run:

```bash
git diff --check
```

Expected: exit 0 with no whitespace errors.

- [ ] **Step 6: Commit the baseline gate**

```bash
git add docs/TELEGRAM_CHARACTERIZATION.md
git commit -m "docs: define Telegram characterization gate"
```

---

### Task 2: Freeze the Rust/TypeScript IPC wire contract

**Files:**
- Create: `src/test/fixtures/telegram-ipc-contract.json`
- Create: `src/ipc-contract.test.ts`
- Modify: `src-tauri/src/model.rs`
- Test: `src/ipc-contract.test.ts`
- Test: `src-tauri/src/model.rs`

**Interfaces:**
- Consumes: current `SearchRequest`, `SearchResponse`, `CatalogProgress`, `AuthSnapshot`, `PlanView`, and `JobRecord` DTOs.
- Produces: one synthetic JSON fixture read by both Rust and TypeScript tests, plus exact assertions for field names, enum spellings, nullability, and safe Telegram numeric IDs.

- [ ] **Step 1: Add a failing Rust test for the shared fixture path**

Append a `#[cfg(test)] mod wire_contract_tests` module to `src-tauri/src/model.rs`:

```rust
#[cfg(test)]
mod wire_contract_tests {
    use super::*;
    use serde_json::Value;

    fn contract() -> Value {
        serde_json::from_str(include_str!(
            "../../src/test/fixtures/telegram-ipc-contract.json"
        ))
        .expect("valid synthetic Telegram IPC contract fixture")
    }

    #[test]
    fn telegram_search_request_wire_names_are_frozen() {
        let mut request: SearchRequest = serde_json::from_value(
            contract()["searchRequest"].clone(),
        )
        .unwrap();
        request.validate().unwrap();
        assert_eq!(request.query, "passport apartment");
        assert_eq!(request.chat_ids, vec![-1001]);
        assert_eq!(request.limit, 500);
        assert!(request.exclude_pinned);
        assert!(request.privacy_scan);
    }
}
```

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml telegram_search_request_wire_names_are_frozen
```

Expected: compilation fails because the fixture does not exist.

- [ ] **Step 2: Add the exact shared JSON fixture**

Create `src/test/fixtures/telegram-ipc-contract.json`:

```json
{
  "searchRequest": {
    "query": "passport apartment",
    "chatIds": [-1001],
    "chatKinds": ["supergroup"],
    "contentKinds": ["photo", "file"],
    "direction": "mine",
    "minDate": "2026-08-01T00:00:00Z",
    "maxDate": "2026-08-31T23:59:59Z",
    "excludePinned": true,
    "privacyScan": true,
    "limit": 500
  },
  "catalogProgress": {
    "phase": "loading",
    "total": 531,
    "processed": 128
  },
  "authSnapshot": {
    "stage": "waiting_for_password",
    "hint": "synthetic hint",
    "qrLink": null
  },
  "searchResponse": {
    "messages": [
      {
        "chatId": -1001,
        "messageId": 14,
        "senderId": 42,
        "senderName": "Synthetic user",
        "sentAt": "2026-08-15T18:00:00Z",
        "isOutgoing": true,
        "contentKind": "photo",
        "preview": "Synthetic passport reminder",
        "privacyFindings": ["identity_document"],
        "albumId": 7001,
        "isPinned": false,
        "deletionReach": "everyone"
      }
    ],
    "returned": 1,
    "truncated": false
  },
  "planView": {
    "id": "11111111-1111-4111-8111-111111111111",
    "operation": "selected_messages",
    "chatTitle": null,
    "targetSenderName": null,
    "summary": {
      "selected": 1,
      "deleteForEveryone": 1,
      "selfOnly": 0,
      "cannotDelete": 0
    },
    "confirmationTier": "low",
    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "createdAt": "2026-08-15T18:01:00Z"
  },
  "jobRecord": {
    "id": "22222222-2222-4222-8222-222222222222",
    "planId": "11111111-1111-4111-8111-111111111111",
    "operation": "selected_messages",
    "targetChatIds": [-1001],
    "status": "completed",
    "total": 1,
    "deleted": 1,
    "skipped": 0,
    "failed": 0,
    "nextBatch": 1,
    "retryAfterSeconds": null,
    "errorCodes": [],
    "createdAt": "2026-08-15T18:01:01Z",
    "updatedAt": "2026-08-15T18:01:02Z"
  }
}
```

- [ ] **Step 3: Complete the Rust response serialization assertions**

In the same Rust test module, add `instant` and deterministic assertions:

```rust
fn instant(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn telegram_response_wire_names_are_frozen() {
    let progress = CatalogProgress {
        phase: "loading",
        total: 531,
        processed: 128,
    };
    assert_eq!(serde_json::to_value(progress).unwrap(), contract()["catalogProgress"]);

    let auth = AuthSnapshot {
        stage: AuthStage::WaitingForPassword,
        hint: Some("synthetic hint".into()),
        qr_link: None,
    };
    assert_eq!(serde_json::to_value(auth).unwrap(), contract()["authSnapshot"]);

    let message = MessageSnapshot {
        chat_id: -1001,
        message_id: 14,
        sender_id: 42,
        sender_name: "Synthetic user".into(),
        sent_at: instant("2026-08-15T18:00:00Z"),
        is_outgoing: true,
        content_kind: ContentKind::Photo,
        preview: "Synthetic passport reminder".into(),
        privacy_findings: vec![cleaner_domain::SensitiveDataKind::IdentityDocument],
        album_id: Some(7001),
        is_pinned: false,
        deletion_reach: cleaner_domain::DeletionReach::Everyone,
    };
    let response = SearchResponse {
        messages: vec![message],
        returned: 1,
        truncated: false,
    };
    assert_eq!(serde_json::to_value(response).unwrap(), contract()["searchResponse"]);

    let plan = PlanView {
        id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        operation: PlanOperation::SelectedMessages,
        chat_title: None,
        target_sender_name: None,
        summary: PlanSummary {
            selected: 1,
            delete_for_everyone: 1,
            self_only: 0,
            cannot_delete: 0,
        },
        confirmation_tier: ConfirmationTier::Low,
        fingerprint: "a".repeat(64),
        created_at: instant("2026-08-15T18:01:00Z"),
    };
    assert_eq!(serde_json::to_value(plan).unwrap(), contract()["planView"]);

    let job = JobRecord {
        id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        plan_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        operation: PlanOperation::SelectedMessages,
        target_chat_ids: vec![-1001],
        status: JobStatus::Completed,
        total: 1,
        deleted: 1,
        skipped: 0,
        failed: 0,
        next_batch: 1,
        retry_after_seconds: None,
        error_codes: Vec::new(),
        created_at: instant("2026-08-15T18:01:01Z"),
        updated_at: instant("2026-08-15T18:01:02Z"),
    };
    assert_eq!(serde_json::to_value(job).unwrap(), contract()["jobRecord"]);
}
```

- [ ] **Step 4: Run the focused Rust wire-contract tests**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml wire_contract_tests
```

Expected: all wire-contract tests pass.

- [ ] **Step 5: Add the TypeScript fixture-consumer test**

Create `src/ipc-contract.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import contract from "./test/fixtures/telegram-ipc-contract.json";
import type { AuthSnapshot, CatalogProgress, JobRecord, PlanView, SearchRequest, SearchResponse } from "./types";

function accept<T>(_value: T): void {}

describe("Telegram IPC contract fixture", () => {
  it("uses the field names consumed by the TypeScript boundary", () => {
    accept<SearchRequest>(contract.searchRequest as SearchRequest);
    accept<CatalogProgress>(contract.catalogProgress as CatalogProgress);
    accept<AuthSnapshot>(contract.authSnapshot as AuthSnapshot);
    accept<SearchResponse>(contract.searchResponse as SearchResponse);
    accept<PlanView>(contract.planView as PlanView);
    accept<JobRecord>(contract.jobRecord as JobRecord);

    expect(Object.keys(contract.searchResponse.messages[0]).sort()).toEqual([
      "albumId", "chatId", "contentKind", "deletionReach", "isOutgoing",
      "isPinned", "messageId", "preview", "privacyFindings", "senderId",
      "senderName", "sentAt"
    ]);
    expect(Object.keys(contract.jobRecord).sort()).toEqual([
      "createdAt", "deleted", "errorCodes", "failed", "id", "nextBatch",
      "operation", "planId", "retryAfterSeconds", "skipped", "status",
      "targetChatIds", "total", "updatedAt"
    ]);
  });

  it("keeps current Telegram identifiers inside the JavaScript safe range", () => {
    const ids = [
      ...contract.searchRequest.chatIds,
      contract.searchResponse.messages[0].chatId,
      contract.searchResponse.messages[0].messageId,
      contract.searchResponse.messages[0].senderId,
      contract.searchResponse.messages[0].albumId,
      ...contract.jobRecord.targetChatIds
    ];
    expect(ids.every(Number.isSafeInteger)).toBe(true);
  });
});
```

- [ ] **Step 6: Run both contract boundaries together**

Run:

```bash
npx vitest run src/ipc-contract.test.ts
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml wire_contract_tests
```

Expected: both commands exit 0.

- [ ] **Step 7: Commit the IPC contract**

```bash
git add src/test/fixtures/telegram-ipc-contract.json src/ipc-contract.test.ts src-tauri/src/model.rs
git commit -m "test: freeze Telegram IPC contract"
```

---

### Task 3: Characterize TDLib normalization with synthetic raw fixtures

**Files:**
- Create: `src-tauri/tests/fixtures/tdlib/message-content.json`
- Create: `src-tauri/tests/fixtures/tdlib/member-statuses.json`
- Create: `src-tauri/tests/fixtures/tdlib/chat-positions.json`
- Modify: `src-tauri/src/live_gateway.rs`
- Test: `src-tauri/src/live_gateway.rs`

**Interfaces:**
- Consumes: private pure helpers `value_i64`, `chat_is_in_catalog`, `message_sender_id`, `role_from_status`, `search_filter`, `content_preview`, and `sensitive_content_text`.
- Produces: synthetic raw TDLib JSON fixtures and table-driven assertions that can later move unchanged into `providers/telegram`.

- [ ] **Step 1: Add a failing fixture loader to the existing `live_gateway` test module**

Add:

```rust
fn fixture(name: &str) -> Value {
    let raw = match name {
        "message-content" => include_str!("../tests/fixtures/tdlib/message-content.json"),
        "member-statuses" => include_str!("../tests/fixtures/tdlib/member-statuses.json"),
        "chat-positions" => include_str!("../tests/fixtures/tdlib/chat-positions.json"),
        _ => panic!("unknown synthetic TDLib fixture"),
    };
    serde_json::from_str(raw).expect("valid synthetic TDLib fixture")
}

#[test]
fn normalizes_every_supported_message_content_fixture() {
    let cases = fixture("message-content");
    assert!(cases.as_array().is_some_and(|cases| !cases.is_empty()));
}
```

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml normalizes_every_supported_message_content_fixture
```

Expected: compilation fails because the fixture files do not exist.

- [ ] **Step 2: Add message-content fixture cases**

Create `message-content.json` as an array of objects with `name`, `content`, `expectedKind`, `expectedPreview`, and `expectedSensitiveText`. Include exactly one synthetic case for each current `ContentKind`: text, photo caption, video, document filename, voice note, audio title, animation caption, sticker emoji, poll question, location coordinates, contact, service message, and unknown/other content.

Use inert values such as `synthetic@example.invalid`, `example.txt`, and coordinates `0.0, 0.0`. Do not use a real filename, location, contact, or credential. The first two entries must be:

```json
[
  {
    "name": "text",
    "content": { "@type": "messageText", "text": { "text": "Synthetic text" } },
    "expectedKind": "text",
    "expectedPreview": "Synthetic text",
    "expectedSensitiveText": "Synthetic text"
  },
  {
    "name": "photo-caption",
    "content": { "@type": "messagePhoto", "caption": { "text": "Synthetic caption" } },
    "expectedKind": "photo",
    "expectedPreview": "Synthetic caption",
    "expectedSensitiveText": "Synthetic caption"
  }
]
```

Extend this array rather than creating a file per content type.

- [ ] **Step 3: Add membership and chat-position fixtures**

Create `member-statuses.json` with creator, administrator-with-delete, administrator-without-delete, member, restricted-member, restricted-nonmember, left, and banned TDLib objects. Each entry includes `expectedRole`, `expectedDelete`, and `expectedLeave`.

Create `chat-positions.json` with visible-main, visible-archive, zero-order-main, and unrelated-list cases. Each entry includes `expectedInCatalog`.

- [ ] **Step 4: Complete the table-driven normalization assertions**

For every message fixture, run:

```rust
let (kind, preview) = content_preview(&case["content"]);
assert_eq!(serde_json::to_value(kind).unwrap(), case["expectedKind"], "{}", case["name"]);
assert_eq!(preview, case["expectedPreview"].as_str().unwrap(), "{}", case["name"]);
assert_eq!(
    sensitive_content_text(&case["content"]),
    case["expectedSensitiveText"].as_str().unwrap(),
    "{}",
    case["name"]
);
```

For membership fixtures, call `role_from_status`. For position fixtures, build a synthetic chat with the supplied `positions` and call `chat_is_in_catalog`.

Add focused assertions that:

- `value_i64` accepts JSON integer and decimal-string forms;
- `message_sender_id` distinguishes user and chat senders;
- a one-kind media filter maps to its current TDLib filter;
- multiple kinds map to `Value::Null`; and
- preview output is capped at 300 characters.

- [ ] **Step 5: Run the live-gateway unit tests**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml live_gateway::tests
```

Expected: all existing and new `live_gateway` tests pass without a network connection.

- [ ] **Step 6: Prove the fixtures contain only synthetic data**

Run:

```bash
rg -n "@gmail|@icloud|@proton|BEGIN .*PRIVATE KEY|api[_ -]?hash|access[_ -]?token|session" src-tauri/tests/fixtures/tdlib
```

Expected: no matches. The `.invalid` email suffix is allowed and does not match this command.

- [ ] **Step 7: Commit TDLib normalization characterization**

```bash
git add src-tauri/tests/fixtures/tdlib src-tauri/src/live_gateway.rs
git commit -m "test: characterize TDLib normalization"
```

---

### Task 4: Characterize Telegram catalog, search, filters, and privacy integration

**Files:**
- Modify: `src-tauri/src/demo_gateway.rs`
- Modify: `src-tauri/src/service.rs`
- Test: `src-tauri/src/demo_gateway.rs`
- Test: `src-tauri/src/service.rs`

**Interfaces:**
- Consumes: `TelegramGateway::chats`, `search`, `chat_by_id`, current `SearchRequest`, and the synthetic gateway catalog.
- Produces: a table-driven query matrix and service-level targeted-refresh/truncation assertions.

- [ ] **Step 1: Add a reusable search-request constructor in `demo_gateway` tests**

Add inside `demo_gateway.rs`'s test module:

```rust
fn request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        chat_ids: Vec::new(),
        chat_kinds: Vec::new(),
        content_kinds: Vec::new(),
        direction: MessageDirection::Any,
        min_date: None,
        max_date: None,
        exclude_pinned: false,
        privacy_scan: false,
        limit: 500,
    }
}
```

- [ ] **Step 2: Write the table-driven search contract**

Add:

```rust
#[test]
fn telegram_search_contract_covers_normalized_filters() {
    tauri::async_runtime::block_on(async {
        let gateway = DemoGateway::new();

        let keyword = request("passport apartment");
        assert_eq!(gateway.search(&keyword).await.unwrap().len(), 1);

        let mut scoped = request("");
        scoped.chat_ids = vec![-1001];
        assert!(gateway.search(&scoped).await.unwrap().iter().all(|m| m.chat_id == -1001));

        let mut outgoing = request("");
        outgoing.direction = MessageDirection::Mine;
        assert!(gateway.search(&outgoing).await.unwrap().iter().all(|m| m.is_outgoing));

        let mut files = request("");
        files.content_kinds = vec![ContentKind::File];
        assert!(gateway.search(&files).await.unwrap().iter().all(|m| m.content_kind == ContentKind::File));

        let mut groups = request("");
        groups.chat_kinds = vec![ChatKind::Supergroup];
        let group_ids = [-1001, -1002, -1003];
        assert!(gateway.search(&groups).await.unwrap().iter().all(|m| group_ids.contains(&m.chat_id)));

        let mut unpinned = request("");
        unpinned.exclude_pinned = true;
        assert!(gateway.search(&unpinned).await.unwrap().iter().all(|m| !m.is_pinned));
    });
}
```

Extend the test with inclusive `min_date`/`max_date`, a result limit of 1, multi-token AND search, sender-name matching, and zero-result cases.

- [ ] **Step 3: Run the focused search contract**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml telegram_search_contract_covers_normalized_filters
```

Expected: pass against current behavior. If a documented filter does not match implementation, record the discrepancy instead of changing production semantics in this task.

- [ ] **Step 4: Add privacy-integration assertions**

Create a `privacy_scan` request with an empty query. Assert results are nonempty, every result has at least one `privacy_findings` entry, the wallet fixture is `CryptoWallet`, and the passport caption is `IdentityDocument`.

- [ ] **Step 5: Add catalog and targeted-refresh service tests**

In `service.rs`, add a test that:

1. calls `snapshot()` once and records `chat_read_counts()`;
2. calls `refresh_chats(vec![-1001, -1001, 304])`;
3. asserts results are deduplicated and title-sorted;
4. asserts the global `chats()` read count did not increase; and
5. removes chat 304 through the gateway, refreshes only 304, and asserts the missing chat is omitted.

Use the current test-only `chat_read_counts` helper and no sleeps.

- [ ] **Step 6: Add search-response truncation characterization**

At service level, send a request with `limit: 1`. Assert `returned == 1` and `truncated == true`, documenting the existing conservative rule that an exactly full page is reported as potentially truncated.

- [ ] **Step 7: Run gateway and service search tests**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml telegram_search_contract_covers_normalized_filters
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml targeted_refresh
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml truncation
```

Expected: all focused tests pass.

- [ ] **Step 8: Commit search characterization**

```bash
git add src-tauri/src/demo_gateway.rs src-tauri/src/service.rs
git commit -m "test: characterize Telegram search and refresh"
```

---

### Task 5: Characterize remediation ordering, authority changes, and batch boundaries

**Files:**
- Modify: `src-tauri/src/demo_gateway.rs`
- Modify: `src-tauri/src/service.rs`
- Test: `src-tauri/src/service.rs`

**Interfaces:**
- Consumes: current `CleanerService` plan/authorize/execute flow and `DemoGateway` mutations.
- Produces: test-only operation recording and capability mutation helpers plus assertions for cleanup-before-leave, no reach downgrade, and bounded deletion calls.

- [ ] **Step 1: Add a failing cleanup-order assertion**

Extend `admin_leave_job_deletes_every_eligible_message_before_removing_membership`:

```rust
let operations = gateway.operation_log().await;
assert_eq!(operations, vec![
    "delete_messages_for_everyone:-1003:31",
    "leave_chat:-1003",
    "remove_chat_for_self:-1003",
]);
```

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml admin_leave_job_deletes_every_eligible_message_before_removing_membership
```

Expected: compilation fails because `operation_log` does not exist.

- [ ] **Step 2: Add test-only operation recording to `DemoGateway`**

Under `#[cfg(test)]`, add:

```rust
operation_log: tokio::sync::Mutex<Vec<String>>,
```

Initialize it in `DemoGateway::new`. Add:

```rust
#[cfg(test)]
async fn record(&self, operation: String) {
    self.operation_log.lock().await.push(operation);
}

#[cfg(test)]
pub(crate) async fn operation_log(&self) -> Vec<String> {
    self.operation_log.lock().await.clone()
}
```

At the start of every destructive fixture method, record a content-free string containing only operation name and immutable numeric IDs. For message deletion, sort the IDs and join them with commas. Never record titles, previews, sender names, or credentials.

- [ ] **Step 3: Run and verify cleanup ordering**

Run the focused admin-leave test again.

Expected: pass with delete, leave, and local removal in that exact order.

- [ ] **Step 4: Add a test-only reach mutation helper**

Add:

```rust
#[cfg(test)]
pub(crate) async fn set_message_reach(
    &self,
    chat_id: i64,
    message_id: i64,
    reach: DeletionReach,
) {
    let mut data = self.data.write().await;
    let message = data.messages.iter_mut().find(|stored| {
        stored.snapshot.chat_id == chat_id && stored.snapshot.message_id == message_id
    }).expect("synthetic message exists");
    message.snapshot.deletion_reach = reach;
}
```

- [ ] **Step 5: Add the no-downgrade regression test**

In `service.rs`, create a selected-message plan for `(101, 1)`, change its reach to `SelfOnly`, authorize and execute it, and wait for terminal status with the current bounded polling pattern. Assert:

```rust
assert_eq!(finished.status, JobStatus::Completed);
assert_eq!(finished.deleted, 0);
assert_eq!(finished.skipped, 1);
assert_eq!(gateway.messages_by_ids(&[(101, 1)]).await.unwrap().len(), 1);
assert!(gateway.operation_log().await.iter().all(|entry| {
    !entry.starts_with("delete_messages_for_everyone:101:")
}));
```

- [ ] **Step 6: Record and assert bounded deletion calls**

Add a `delete_batch_sizes: Mutex<Vec<usize>>` test-only field. Seed 205 synthetic everyone-deletable outgoing messages in one chat through a test-only `append_messages` helper. Prepare the frozen selection and execute it. Assert the gateway received `[100, 100, 5]`, every call is scoped to one chat, and the completed job reports 205 deleted.

The helper constructs messages with `preview: "Synthetic batch message"`, no privacy findings, no album, and sequential positive message IDs. It does not accept arbitrary preview text.

- [ ] **Step 7: Run all service safety tests**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml service::tests
```

Expected: all service tests pass, including cancellation, grants, restart, leave, ordering, downgrade, and batch assertions.

- [ ] **Step 8: Commit remediation characterization**

```bash
git add src-tauri/src/demo_gateway.rs src-tauri/src/service.rs
git commit -m "test: characterize Telegram cleanup safety"
```

---

### Task 6: Freeze encrypted-store compatibility and content-free persistence

**Files:**
- Modify: `src-tauri/src/secure_store.rs`
- Modify: `src-tauri/src/service.rs`
- Test: `src-tauri/src/secure_store.rs`
- Test: `src-tauri/src/service.rs`

**Interfaces:**
- Consumes: `RTRCT01`, `RTRCT02`, `SecureJobStore`, `PersistedState`, and current restart policy.
- Produces: explicit legacy-unbound coverage, content-free serialized-state assertions, and restart assertions covering every non-idempotent broad operation.

- [ ] **Step 1: Add an `RTRCT01` legacy-unbound test**

Inside `secure_store.rs` tests, construct an authenticated legacy payload using the existing private constants and AES-GCM implementation:

```rust
#[test]
fn loads_legacy_unbound_job_store_and_marks_it_for_safe_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("jobs.enc");
    let key = [0x2a; KEY_LENGTH];
    let plaintext = serde_json::to_vec(&PersistedState::default()).unwrap();
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce_bytes = [0x11; NONCE_LENGTH];
    let nonce = AesNonce::from(nonce_bytes);
    let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref()).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(LEGACY_UNBOUND_MAGIC);
    bytes.extend_from_slice(&nonce_bytes);
    bytes.extend_from_slice(&ciphertext);
    fs::write(&path, bytes).unwrap();

    let store = SecureJobStore::with_test_key_and_profile(path, key, b"telegram-production");
    let state = store.load().unwrap();
    assert!(state.plans.is_empty());
    assert!(state.jobs.is_empty());
    assert!(store.loaded_legacy_unbound());
}
```

- [ ] **Step 2: Run the focused legacy-store test**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml loads_legacy_unbound_job_store
```

Expected: pass.

- [ ] **Step 3: Add a persisted-state content allowlist test**

In `service.rs`, create a plan from a `MessageSnapshot` whose preview is `SYNTHETIC_CONTENT_MUST_NOT_PERSIST`, save and reload it, serialize the reloaded `PersistedState`, and assert:

```rust
let serialized = serde_json::to_string(&reloaded).unwrap();
assert!(!serialized.contains("SYNTHETIC_CONTENT_MUST_NOT_PERSIST"));
for forbidden in [
    "preview", "text", "caption", "fileName", "attachment",
    "apiHash", "password", "authCode"
] {
    assert!(!serialized.contains(&format!("\"{forbidden}\"")));
}
```

Also assert required recovery data remains: plan ID, fingerprint, operation, target IDs, expected reach, counts, next batch, error codes, and timestamps.

- [ ] **Step 4: Expand non-idempotent restart characterization**

Convert the existing broad-restart setup into a helper and table-drive:

```text
clear_history
clear_history_and_leave
remove_chat_for_self
delete_by_sender
delete_group
```

For each operation, assert a queued/running persisted job becomes failed or partial according to whether its deleted count is zero, receives `restart_requires_new_review`, and makes no fixture mutation call. Keep selected-message and own-message frozen-ID resume behavior as separate positive tests.

- [ ] **Step 5: Run the secure-store and restart suites**

Run:

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml secure_store::tests
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml restart
```

Expected: both focused suites pass.

- [ ] **Step 6: Commit persistence characterization**

```bash
git add src-tauri/src/secure_store.rs src-tauri/src/service.rs
git commit -m "test: freeze Telegram persistence safety"
```

---

### Task 7: Characterize Telegram authentication and frontend selection/review behavior

**Files:**
- Create: `src/components/AuthGate.test.tsx`
- Modify: `src/App.test.tsx`
- Test: `src/components/AuthGate.test.tsx`
- Test: `src/App.test.tsx`

**Interfaces:**
- Consumes: current `AuthSnapshot`, `AuthCommand`, album selection, hidden-selection count, plan review, owner authorization, and targeted refresh.
- Produces: UI tests that can later rerun against provider-aware account/auth state and action descriptors.

- [ ] **Step 1: Add an auth-stage table test**

Create `src/components/AuthGate.test.tsx`. Mock `@retract/api`, then table-drive:

```ts
const cases = [
  ["waiting_for_phone", "Phone number", "submit_phone"],
  ["waiting_for_email_address", "Email address", "submit_email_address"],
  ["waiting_for_email_code", "Email code", "submit_email_code"],
  ["waiting_for_code", "Telegram sign-in code", "submit_code"],
  ["waiting_for_password", "Two-step verification password", "submit_password"]
] as const;
```

For each stage, render `AuthGate`, enter `synthetic-value`, submit, and assert `api.submitAuth` receives the expected command and value exactly once. Assert `onRefresh` runs only after successful submission.

- [ ] **Step 2: Add secret-clearing and error-state tests**

Make `submitAuth` reject for email-code, Telegram-code, and password stages. Assert the input is cleared and an alert shows the normalized error. For phone/email-address rejection, assert the current non-secret value remains for correction.

Add QR tests asserting `requestQrAuth` is used only from the phone stage and that `waiting_for_other_device` with a QR link renders an image named `Telegram device-link QR code`.

- [ ] **Step 3: Run the focused auth tests**

Run:

```bash
npx vitest run src/components/AuthGate.test.tsx
```

Expected: all auth-stage, error, and QR tests pass.

- [ ] **Step 4: Add album atomicity to `App.test.tsx`**

Add:

```ts
it("selects and clears every visible message in a Telegram album atomically", async () => {
  render(<App />);
  await screen.findByText("Search every chat");
  fireEvent.click(await screen.findByText("cedar_research_notes.pdf · 4.8 MB"));
  expect(screen.getByText("2 messages selected")).toBeInTheDocument();
  expect(screen.getByText("cedar_research_notes.pdf · 4.8 MB").closest("article"))
    .toHaveClass("is-selected");
  expect(screen.getByText("Whiteboard with customer email list").closest("article"))
    .toHaveClass("is-selected");

  fireEvent.click(screen.getByText("Whiteboard with customer email list"));
  expect(screen.getByText("0 messages selected")).toBeInTheDocument();
});
```

- [ ] **Step 5: Add hidden-selection retention**

Select `Project Cedar launch credentials moved to the vault.`, switch the content filter to `Media`, and assert the impact panel says `1 selection outside this result view`, the Review button remains enabled, and `Clear all` removes the hidden selection.

- [ ] **Step 6: Add stale-search response rejection**

Mock two `api.search` calls with deferred promises. Enter a first query, advance fake timers past the 120 ms debounce, enter a second query, and resolve the second response before the first. Assert only the second result appears. Resolve the first and assert it does not replace the current results.

Use `vi.useFakeTimers({ shouldAdvanceTime: true })` only within this test and restore real timers in `finally`.

- [ ] **Step 7: Add review/authorization ordering**

Spy on `prepareSelection`, `authorizePlan`, and `execute`. Select a message and open review. Assert no authorization or execution occurred. Submit and assert:

```ts
expect(api.authorizePlan).toHaveBeenCalledTimes(1);
expect(api.execute).toHaveBeenCalledTimes(1);
expect(vi.mocked(api.authorizePlan).mock.invocationCallOrder[0])
  .toBeLessThan(vi.mocked(api.execute).mock.invocationCallOrder[0]);
```

Cancel a second dialog with Escape and assert neither call count increases.

- [ ] **Step 8: Run focused frontend characterization**

Run:

```bash
npx vitest run src/components/AuthGate.test.tsx src/App.test.tsx
```

Expected: all focused UI tests pass without changing production components.

- [ ] **Step 9: Commit frontend characterization**

```bash
git add src/components/AuthGate.test.tsx src/App.test.tsx
git commit -m "test: characterize Telegram UI workflows"
```

---

### Task 8: Run the complete gate and document the refactor baseline

**Files:**
- Modify: `docs/TEST_PLAN.md`
- Modify: `docs/TELEGRAM_CHARACTERIZATION.md`
- Read: `scripts/verify-production-bundle.sh`
- Read: `.github/workflows/secure-build.yml`

**Interfaces:**
- Consumes: every test and fixture introduced by Tasks 1–7.
- Produces: a verified automated baseline and explicit CI/release requirements for provider-foundation work.

- [ ] **Step 1: Add the characterization command block to `docs/TEST_PLAN.md`**

Immediately after the existing automated gate, add:

````markdown
### Telegram provider-refactor characterization

Before changing Telegram-facing domain or IPC types, run:

```sh
npx vitest run src/ipc-contract.test.ts src/components/AuthGate.test.tsx src/App.test.tsx src/api.test.ts
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml wire_contract_tests
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml live_gateway::tests
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml service::tests
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml secure_store::tests
```

These tests are compatibility gates. A provider migration may update their type names only in the same reviewed change that supplies an explicit old-to-new wire migration and proves equivalent Telegram behavior.
````

- [ ] **Step 2: Run formatting and static checks**

Run:

```bash
git diff --check
cargo fmt --manifest-path crates/cleaner-domain/Cargo.toml -- --check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path crates/cleaner-domain/Cargo.toml --all-targets -- -D warnings
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Expected: every command exits 0 with no warnings treated as errors.

- [ ] **Step 3: Run the complete host test/build gate**

Run:

```bash
npm run check
```

Expected: TDLib verification, all Vitest tests, TypeScript compilation, Vite production build, domain tests, and Tauri backend tests exit 0.

- [ ] **Step 4: Verify fixture isolation in the production bundle**

Run:

```bash
npm run verify:production-bundle -- --existing
```

Expected: exit 0 and no fixture adapter or synthetic catalog in `dist`.

- [ ] **Step 5: Run the containerized architecture gate**

Run:

```bash
npm run container:check
```

Expected: the Docker BuildKit checks target succeeds for supported build architectures. If Docker is unavailable, do not mark this step complete; report the environmental blocker.

- [ ] **Step 6: Record the automated baseline facts**

Append to `docs/TELEGRAM_CHARACTERIZATION.md`:

```markdown
## Automated baseline command

The canonical local command is `npm run check`; the canonical isolated command is `npm run container:check`. Pull requests that change provider-facing models must include both command outcomes in the PR verification section and must identify any live test-DC cases rerun because destructive behavior changed.
```

Do not write a date-specific pass count into the permanent document because test counts will grow. Put exact counts and commit IDs in the pull request verification record.

- [ ] **Step 7: Run the public-repository safety check**

Run:

```bash
npm run check:public-repo
```

Expected: exit 0 with no private artifacts, credentials, sessions, databases, or archives detected.

- [ ] **Step 8: Commit the completed characterization gate**

```bash
git add docs/TEST_PLAN.md docs/TELEGRAM_CHARACTERIZATION.md
git commit -m "docs: record Telegram refactor baseline"
```

- [ ] **Step 9: Verify final repository state**

Run:

```bash
git status --short --branch
git log --oneline -8
```

Expected: the working tree is clean and the plan produced focused commits for the gate document, IPC contract, TDLib normalization, search/refresh, cleanup safety, persistence safety, frontend workflows, and final baseline documentation.

## Exit criteria

This plan is complete only when:

- all Task 8 verification commands available in the development environment exit 0;
- unavailable Docker or live test-DC checks are explicitly reported rather than assumed;
- synthetic fixture scans and the public-repository check find no private artifacts;
- production behavior files have no semantic changes;
- every new test is deterministic and passes without Telegram network access;
- `docs/TELEGRAM_CHARACTERIZATION.md` and `docs/TEST_PLAN.md` describe the gate; and
- reviewers can point to an automated assertion for IPC shape, TDLib mapping, search/filter behavior, capability change, cleanup ordering, batch bounds, persistence compatibility, auth flow, selection, review, and targeted refresh.
