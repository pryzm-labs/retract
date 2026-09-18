# Discord Archive Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make imported Discord archives first-class searchable sources and delete the archive owner's exact historical messages using an explicitly verified Discord user session.

**Architecture:** Keep archive inventory/query evidence separate from live session authority. A zeroizing Discord session owner validates and optionally persists one account-bound credential; browser adapters capture only a Discord API Authorization header from isolated temporary profiles; the existing reviewed-plan lifecycle executes exact archive locators through a bounded rate-aware REST client.

**Tech Stack:** Rust 2024, Tauri 2, Tokio, Reqwest/rustls, Tungstenite, React 19, TypeScript 6, Vitest, SQLCipher archive storage, macOS Keychain vault.

**Spec:** `docs/superpowers/specs/2026-09-15-discord-remediation-design.md`

## Global Constraints

- Discord remediation is explicitly unsupported normal-user automation and must display account-risk disclosure before capture and destructive execution.
- Only messages attributed to the imported archive owner and represented by exact archive channel/message locators may execute.
- No token, cookie, password, message content, archive path or Discord response body enters frontend state, logs, diagnostics or durable job data.
- Tokens are memory-only by default; Keychain persistence is opt-in and uses the consolidated cached vault.
- Automatic capture uses new temporary profiles and never reads an existing browser profile.
- Chromium-family and Firefox are first-class automatic providers; manual token entry is always available for unsupported browsers.
- All tests are synthetic/offline; no test contacts Discord or launches a real installed browser.

---

### Task 1: Account-bound Discord secret lifecycle

**Files:**
- Create: `src-tauri/src/providers/discord/session.rs`
- Create: `src-tauri/src/providers/discord/session_tests.rs`
- Modify: `src-tauri/src/providers/discord/mod.rs`
- Modify: `src-tauri/src/secure_store.rs`
- Modify: `src-tauri/src/secure_store/vault.rs`
- Modify: `src-tauri/src/secure_store/vault_tests.rs`
- Modify: `src-tauri/Cargo.toml`

**Interfaces:**
- Produces `DiscordCredential { account_id: DiscordId, token: Zeroizing<String> }` with a private token field.
- Produces `DiscordSessionOwner::{status, submit_manual, install_captured, forget, shutdown}`.
- Produces a testable `DiscordIdentityClient` boundary returning `VerifiedDiscordIdentity`.
- Produces `secure_store::{load_discord_credential, save_discord_credential, forget_discord_credential}`.

- [ ] **Step 1: Write failing vault tests** for frozen v1/v2 compatibility, one bounded namespaced Discord credential, opt-in save, exact account/token round trip, forget preserving all Telegram/archive keys, unknown-entry rejection, uncertain-write reconciliation and one-read denial caching.
- [ ] **Step 2: Run the focused vault tests** with `CARGO_TARGET_DIR=/Users/aaron/dev/tg-cleaner/src-tauri/target cargo test --locked --manifest-path src-tauri/Cargo.toml secure_store::vault_tests`; confirm failure because the Discord entry APIs do not exist.
- [ ] **Step 3: Implement vault v3 encoding** with closed entry names, length-prefixed variable credential bytes, strict decimal account/token bounds and zeroization; preserve v1/v2 bytes unless a Discord credential mutation is explicitly committed.
- [ ] **Step 4: Write failing session tests** proving whitespace normalization, control/size/Bot/Bearer rejection, `GET /users/@me` identity parsing, archive-owner mismatch rejection, no token in Debug/errors/serialized status, replacement zeroization ownership, invalid-session forget and shutdown.
- [ ] **Step 5: Implement the session owner and injected HTTP identity client** using `reqwest` with rustls, a 15-second timeout, fixed safe errors and no response-body propagation.
- [ ] **Step 6: Run focused and full Rust tests**, then commit `feat: add account-bound Discord session vault`.

### Task 2: Browser-provider token capture

**Files:**
- Create: `src-tauri/src/providers/discord/browser/mod.rs`
- Create: `src-tauri/src/providers/discord/browser/discovery.rs`
- Create: `src-tauri/src/providers/discord/browser/chromium.rs`
- Create: `src-tauri/src/providers/discord/browser/firefox.rs`
- Create: `src-tauri/src/providers/discord/browser/protocol.rs`
- Create: `src-tauri/src/providers/discord/browser/tests.rs`
- Modify: `src-tauri/src/providers/discord/mod.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`
- Modify: `scripts/check-provider-boundaries.mjs`
- Modify: `THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Produces `BrowserDescriptor { id, display_name, family }` and `BrowserFamily::{ChromiumCdp, FirefoxBidi}`.
- Produces `BrowserTokenCapture::{discover, capture}` and `CaptureProgress::{Launching, WaitingForLogin, Verifying, Complete}`.
- Consumes `DiscordSessionOwner::install_captured` without returning the token to IPC.

- [ ] **Step 1: Write failing discovery tests** over injected filesystem/PATH probes for Chrome, Edge, Brave, Arc, Vivaldi, Opera, Chromium and Firefox on macOS/Linux/Windows; assert deterministic deduplication and no existing profile inspection.
- [ ] **Step 2: Write failing protocol-fixture tests** that feed synthetic CDP and BiDi JSON events and accept only a bounded non-Bot/non-Bearer Authorization header from `https://discord.com/api/`; reject lookalikes, other headers, cookies, controls and malformed events.
- [ ] **Step 3: Implement protocol-neutral discovery and event parsing** with no process launch yet; run focused tests to green.
- [ ] **Step 4: Write failing lifecycle tests** using injected child/WebSocket transports for private temporary profile creation, loopback-only endpoints, exact login URL, cancel/timeout, child termination, cleanup and no protocol/token logging.
- [ ] **Step 5: Implement Chromium CDP and Firefox BiDi adapters** behind one interface using `tokio-tungstenite`; isolate browser-specific command/event handling in their files and return only a zeroizing captured value to the session owner.
- [ ] **Step 6: Extend provider-boundary and dependency checks**, document new licenses/checksums, run strict fmt/Clippy and offline protocol tests, then commit `feat: add browser-independent Discord sign-in`.

### Task 3: Exact archive-driven deletion engine

**Files:**
- Create: `src-tauri/src/providers/discord/remediation.rs`
- Create: `src-tauri/src/providers/discord/remediation_tests.rs`
- Create: `src-tauri/src/providers/discord/http.rs`
- Modify: `src-tauri/src/providers/discord/locators.rs`
- Modify: `src-tauri/src/providers/discord/mod.rs`
- Modify: `src-tauri/src/persistence/archive/query.rs`
- Modify: `src-tauri/src/persistence/archive/worker.rs`
- Modify: `crates/retract-domain/src/actions.rs`

**Interfaces:**
- Produces `DiscordRemediationProvider: RemediationProvider` and provider-owned recipe `discord.delete_messages.v1`.
- Produces `DiscordDeleteClient::delete(channel_id, message_id, token) -> DeleteResponse`.
- Consumes archive `resolve` results and the matching verified `DiscordSessionOwner` credential.

- [ ] **Step 1: Write failing plan tests** proving only owner-authored Discord content from one ready archive scope is selectable; exact channel/message IDs, sorted target order, pacing and risk confirmation are fingerprinted; mismatched accounts/sources and non-message locators fail before HTTP.
- [ ] **Step 2: Implement action descriptors, recipe validation and immutable plan preparation** through the existing reviewed lifecycle without provider-name branching in the UI.
- [ ] **Step 3: Write failing HTTP outcome tests** against a local synthetic server for 204, 404, 401, 403, 429 route/global delays, bounded `retry_after`, network/5xx retry exhaustion, cancellation and ambiguous in-flight outcomes; assert request path/method/headers but never print the test token.
- [ ] **Step 4: Implement the rate-aware REST client** with per-channel serialization, four-channel bound, 1.5-second jittered floor, 20-rps account ceiling, global pause, bounded retries and fixed diagnostics.
- [ ] **Step 5: Write failing durable lifecycle tests** for progress after every confirmed/absent result, crash/restart cursor, 401 session invalidation, pause/resume/cancel, no duplicate scheduling and content-free encrypted state.
- [ ] **Step 6: Wire the Discord reviewed lifecycle and verification semantics**, run focused/full Rust tests, then commit `feat: delete exact Discord archive messages`.

### Task 4: Production archive/source/session IPC

**Files:**
- Create: `src-tauri/src/providers/discord/commands.rs`
- Create: `src-tauri/src/providers/discord/application.rs`
- Create: `src-tauri/src/providers/discord/commands_tests.rs`
- Modify: `src-tauri/src/compatibility/commands_v2.rs`
- Modify: `src-tauri/src/compatibility/model_v2.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/provider_service.rs`
- Modify: `src/api-contract.ts`
- Modify: `src/providers/api.ts`
- Modify: `src/providers/contract.ts`
- Modify: `src/types.ts`
- Modify: `src/ipc-contract.test.ts`
- Modify: `src/api.test.ts`

**Interfaces:**
- Produces version-2 commands `list_sources_v2`, `select_archive_v2`, `start_discord_import_v2`, `get_discord_import_v2`, `cancel_discord_import_v2`, `get_discord_session_v2`, `discover_discord_browsers_v2`, `start_discord_browser_auth_v2`, `submit_discord_token_v2`, and `forget_discord_session_v2`.
- Produces closed TypeScript DTOs/methods with no credential-returning response.

- [ ] **Step 1: Write failing Rust command tests** using the real Tauri test router for strict request versions/fields, native-selected regular files, import progress, source activation, stale contexts, account match and no secret/path response fields.
- [ ] **Step 2: Implement runtime-owned Discord application/source activation**, retaining import/session/browser/deletion workers through shutdown and preserving Telegram service replacement semantics.
- [ ] **Step 3: Write failing TypeScript contract tests** for every DTO/state/error branch, large Discord IDs as strings, token request one-way behavior and unknown-field rejection.
- [ ] **Step 4: Implement and register the production commands and frontend adapter**, then run Rust IPC and TypeScript API tests to green.
- [ ] **Step 5: Run full backend/frontend tests and commit** `feat: expose Discord archive remediation API`.

### Task 5: First-class Discord source and remediation UI

**Files:**
- Create: `src/components/SourceSetupDialog.tsx`
- Create: `src/components/DiscordConnectionDialog.tsx`
- Create: `src/components/DiscordImportProgress.tsx`
- Create: `src/components/DiscordConnectionDialog.test.tsx`
- Create: `src/components/SourceSetupDialog.test.tsx`
- Modify: `src/components/ConnectionSettingsDialog.tsx`
- Modify: `src/components/Sidebar.tsx`
- Modify: `src/components/ImpactPanel.tsx`
- Modify: `src/components/JobDetails.tsx`
- Modify: `src/components/ConfirmDialog.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/styles.css`
- Modify: `src/api.fixture.ts`

**Interfaces:**
- Consumes Task 4 `RetractApi` source/import/session methods.
- Produces archive-only onboarding, source switching, Discord authentication, manual fallback, review and job controls in the existing visual system.

- [ ] **Step 1: Write failing component tests** for Telegram-or-Discord first run, local ZIP import/cancel/progress, immediate archive search, browser choice, always-visible manual fallback, token masking/reveal, account mismatch, remember opt-in and forget.
- [ ] **Step 2: Implement source setup and Discord connection dialogs** with accessible labels, keyboard/focus behavior, explicit risk consent and no token retention after submission/unmount.
- [ ] **Step 3: Write failing application tests** for source/context switching clearing stale state, exact Discord selection/review, immediate pending rows, duplicate-run prevention and job counters including absent/skipped/uncertain.
- [ ] **Step 4: Integrate Discord sources into App/Sidebar/Impact/Confirm/Jobs** using backend descriptors, preserving Telegram behavior and targeted refresh.
- [ ] **Step 5: Run frontend tests/build and commit** `feat: add Discord cleanup workflow`.

### Task 6: Documentation, hardening and release gate

**Files:**
- Modify: `README.md`
- Modify: `docs/THREAT_MODEL.md`
- Modify: `docs/TEST_PLAN.md`
- Modify: `PLAN.md`
- Modify: `scripts/check-public-repo.mjs`
- Modify: `scripts/check-provider-boundaries.mjs`
- Modify: `Dockerfile`
- Modify: `.github/workflows/secure-build.yml`

**Interfaces:**
- Produces user setup/testing instructions and CI gates for the complete Discord feature.

- [ ] **Step 1: Document the unsupported-account risk**, archive request/import, automatic isolated-browser sign-in, manual token fallback, memory/Keychain behavior, forget flow, dry review, pacing and disposable-account live test procedure.
- [ ] **Step 2: Extend threat model and repository checks** for credential capture, browser automation endpoint exposure, origin confusion, profile leakage, token logs, archive/account mismatch, rate-limit abuse and ambiguous deletion outcomes.
- [ ] **Step 3: Add Docker/CI dependency and architecture checks** without bundling browser binaries or retaining additional BuildKit caches.
- [ ] **Step 4: Run fresh verification**: `npm test`, `npm run build`, `npm run check:public-repo`, `npm run check:provider-boundaries`, all Rust workspace/crate tests with strict fmt/Clippy, `git diff --check`, ARM64 Docker checks and AMD64 Docker checks.
- [ ] **Step 5: Inspect the production bundle and diff**, confirm no token strings/private fixtures/generated browser profiles, update the roadmap checkbox, and commit `docs: document Discord archive deletion`.
