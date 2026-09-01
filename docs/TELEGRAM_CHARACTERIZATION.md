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
