# Telegram behavior contract

This document is the regression gate for moving Telegram behind Retract's provider architecture. The migration passes only when the automated contract below and the applicable disposable test-DC matrix in `docs/TEST_PLAN.md` produce the same observable behavior before and after the refactor.

## Automated contract

- IPC response and request field names remain compatible until an explicitly versioned migration changes both Rust and TypeScript.
- Telegram chat/message/sender IDs retain the current signed numeric, JavaScript-safe wire behavior throughout Phase 0.
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
- The UI preserves album atomicity, hidden selections, review-before-execute, current job status/retry/deleted presentation, cancellation, and targeted reconciliation.

## Current cleanup-job presentation boundary

Phase 0 recent-job rows render the operation, every current status, a rate-limit retry countdown when present, and the deleted count when nonzero. The current UI does not render the job record's `skipped`, `failed`, or `errorCodes` diagnostics; those fields remain available through the backend job record and content-free log. Characterization tests assert that omitted values are not folded into or misrepresented by the visible status/deleted copy.

Before provider UI generalization, Retract must add an accessible full-result presentation for skipped and failed counters plus safe structured diagnostics. That future work is a prerequisite for claiming that provider-neutral job results expose complete outcome diagnostics; Phase 0 freezes only the narrower current surface.

## Phase boundary and required provider-foundation gate

This Phase 0 contract deliberately tests only Telegram's current numeric IDs inside JavaScript's safe-integer range. It does not prove the approved opaque-ID design. Before the first provider-foundation change modifies any production identifier type, that change must add a behavior-level RED harness with opaque strings and provider-native values greater than JavaScript's safe-integer range across selection, planning/fingerprints, job tracking, encrypted persistence/recovery, Rust/TypeScript IPC, and dirty targeted refresh/reconciliation. The Phase 0 numeric test may not be treated as sufficient evidence for that migration.

## Existing encrypted-store preservation contract

Today `SecureJobStore` writes and syncs a new encrypted temporary file before replacing `jobs.enc`. The automated contract forces a temporary-write failure and proves that the last authenticated original remains byte-for-byte readable. Once replacement succeeds, Retract retains no application-managed backup or general rollback copy. Future provider/store migration must write and verify a separate new store, keep the original until verification succeeds, and atomically switch only after that success.

## Live test-DC contract

Use disposable identities and execute the search/catalog, destructive, confirmation/abuse, reliability, and accessibility sections of `docs/TEST_PLAN.md`. Store no account credentials, chat exports, message contents, or TDLib databases in the repository. Record only commit IDs, environment versions, role/capability labels, operation names, provider result codes, counts, and pass/fail outcomes.

## Refactor parity rule

Run this gate on the last pre-refactor commit and on the Telegram-provider migration commit. A changed result requires either a defect fix approved as a separate behavior change or a revision to the architecture design; it may not be dismissed as an internal refactor difference.

## Automated baseline command

The canonical local command is `npm run check`; the canonical isolated command is `npm run container:check`. Pull requests that change provider-facing models must include both command outcomes in the PR verification section and must identify any live test-DC cases rerun because destructive behavior changed.
