# Discord archive remediation

**Date:** 2026-09-15

**Status:** Approved for implementation on 2026-09-15

**Base:** `b8e004a7aa8ef1b1480c42df44eb07c2eb99c34e` — Discord archive import merged in PR #25.

**Parents:** [Multi-provider architecture](2026-09-01-multi-provider-architecture-design.md) and [Discord archive import](2026-09-07-discord-archive-import-design.md).

## 1. Decision and explicit policy amendment

Retract will use an imported Discord Data Package as the authoritative inventory of the current user's message channel/message IDs and may use a verified Discord user session token to delete those messages through Discord's REST endpoint. This deliberately supersedes only the parent design's prohibition on Discord user-token automation. It does not weaken any other local-first, review, identity, secret-handling, persistence, or truthful-result requirement.

Discord does not document an OAuth scope for deleting a normal user's historical messages. This integration therefore uses unsupported normal-user automation and may violate Discord's Terms or lead to account restriction. Retract must disclose that risk before token capture and again before the first destructive run. It must never describe this as an official Discord integration.

The feature deletes only messages attributed to the archive owner and listed in the imported archive. It does not search Discord, scrape unrelated channels, delete other users' messages, send messages, join servers, or broaden targets using live API responses.

## 2. Product flow

Discord is a first-class source and does not require Telegram configuration.

1. The first-run/source UI offers **Connect Telegram** and **Import Discord archive**.
2. Discord import uses the existing bounded reader and encrypted archive store. The UI shows content-free phase/counter progress and never uploads the ZIP.
3. An imported Discord source is immediately searchable and privacy-scannable without live authentication.
4. To delete, the user chooses **Sign in with Discord** or **Enter token manually**.
5. Automatic sign-in launches a visible supported browser with a new temporary profile. Retract observes only requests to Discord API hosts and retains only a plausible non-Bot, non-Bearer authorization header. It never reads an existing browser profile or captures passwords.
6. Automatic capture is browser-provider based: Chromium-family browsers use CDP and Firefox uses WebDriver BiDi. Unsupported browsers, including Safari where request headers are unavailable through supported automation APIs, use manual token entry.
7. The token is verified through `GET /users/@me`. Its Discord user ID must exactly match the archive account identity before remediation becomes available.
8. The token is memory-only by default. **Remember in Keychain** is explicit opt-in. **Forget Discord session** zeroizes the in-memory value and removes the persisted value.
9. Search, privacy scan, filters, selection and review use the existing normalized UI. Discord actions clearly state that they delete the selected current user's Discord messages.
10. A run is dry/review-only until the user acknowledges irreversibility, accepts the unsupported-integration warning, and completes the existing device-owner authorization gate.

## 3. Browser-independent capture boundary

The application core depends on a `BrowserTokenCapture` interface, not a browser name or protocol. Providers return a bounded descriptor, launch an isolated child process, report coarse progress, and return a zeroizing token or a fixed safe error.

Initial automatic providers:

- Chromium CDP: Google Chrome, Microsoft Edge, Brave, Arc, Vivaldi, Opera and Chromium on supported platforms.
- Firefox WebDriver BiDi: Firefox stable/beta/developer editions where the built-in remote agent exposes the required network event.

The provider launcher binds automation to loopback, chooses a private temporary profile, disables password saving where supported, opens only `https://discord.com/login`, and terminates the child after capture/cancel/timeout. Profile cleanup is best-effort after the browser exits and never follows symlinks. Protocol messages, request bodies, cookies and headers other than the selected Authorization value are not logged or returned.

Automatic capture uses an allowlist of HTTPS origins under `discord.com/api/`; lookalike domains, redirects outside Discord, Bot/Bearer values, empty values, controls and oversized tokens are rejected. The captured token is verified before the temporary profile is removed from the active flow.

Manual entry is always present, password-masked, paste-friendly, non-autocompleting and frontend-memory-only until submission. The frontend never receives a valid token back from Rust.

## 4. Secret lifecycle

`DiscordSessionOwner` is the sole in-process owner of usable Discord tokens. Values use `Zeroizing<String>` and are never cloneable DTO fields. It exposes only status, verified account identity, capture/validation commands and a request-scoped authenticated HTTP operation.

The macOS consolidated vault gains one namespaced optional Discord entry containing a bounded versioned pair of account ID and token. This preserves the single Keychain-item/cache behavior that prevents prompt loops. Existing `RTRCTV1`/`RTRCTV2` records remain readable byte-for-byte until a Discord token is explicitly remembered. Unknown entries still fail closed. Non-macOS builds use the existing private authenticated local-secret mechanism until platform keyrings are implemented.

On startup, persisted Discord credentials are loaded lazily only when a matching Discord source is activated. A failed/denied load is cached for that application lifetime. A rejected or mismatched token is forgotten. Logout, explicit forget and terminal shutdown zeroize memory; explicit forget also commits a vault without the Discord entry.

No token, password, cookie, authorization header, archive path, message body, attachment name or Discord response body may appear in frontend state, durable job data, normal logs, panic strings, error DTOs or crash diagnostics.

## 5. Planning and deletion engine

Discord message locators already bind account, source, channel and message IDs. The archive query service resolves selected references before planning. The plan freezes exact sorted targets, account/source scope, recipe version, expected effect, pacing policy and confirmation requirements. Archive records cannot become executable until a matching live identity is verified.

The executor sends `DELETE /api/v10/channels/{channel_id}/messages/{message_id}` with the verified user token. It processes each channel serially and permits bounded cross-channel concurrency. A global limiter and per-route rate state honor Discord rate-limit headers and `retry_after`; jitter is applied between successful requests. The initial conservative floor is 1.5 seconds per channel and four channels, with an account-wide ceiling of 20 requests per second. These are maximum rates, not targets.

Outcome rules:

- `204`: confirmed deleted.
- `404`: confirmed absent and complete.
- `401`: stop all workers, forget the invalid session, and report authentication required.
- `403`: permanently skipped for this run with a fixed `permission_changed` diagnostic.
- `429`: pause the applicable bucket/global workers for the server-provided bounded delay, then retry.
- network failure or `5xx`: bounded exponential retry; an exhausted or interrupted request is `uncertain`, never confirmed.
- all other statuses: fixed safe failure without preserving the response body.

Progress is persisted after every confirmed/absent item using the existing content-free reviewed job store. Restart resumes only exact frozen IDs whose prior outcome is neither confirmed nor permanently skipped, after a fresh matching-token check and new user review when the previous request may have been ambiguous. Pause, resume and cancel stop scheduling new requests; cancellation cannot relabel an in-flight call.

The imported archive remains immutable evidence. Successful deletion does not erase the local archive automatically; the UI marks confirmed targets locally and offers a separate **Remove imported source from Retract** action.

## 6. Application and IPC boundaries

Add production commands for Discord source listing/import progress/cancel, source activation, session status, browser discovery/capture, manual token validation/remember/forget, plan preparation and job control. Every request uses contract version 2 and a current scope/context where identity exists. Paths come only from the native file picker and are opened read-only with the existing import preflight; arbitrary frontend paths are rejected.

Discord provider registration supplies the existing archive-backed query port, payload validator, action descriptors and reviewed remediation lifecycle. The React API decodes closed DTOs and rejects unknown fields, providers, states and error codes. Provider/source switching increments context generation and clears stale searches, selections, plans and polling just as Telegram account changes do.

## 7. UI behavior

The provider/source selector distinguishes Telegram live sources from Discord archive sources. Discord rows show imported/ready, authentication status and message count without implying that archive evidence is live.

The Discord connection dialog has:

- risk disclosure and explicit consent;
- automatic **Sign in with Discord** with detected-browser choice;
- always-visible **Enter token manually** fallback;
- password-masked token field with reveal/paste controls;
- verified avatar/name/handle and exact archive-account match state;
- opt-in **Remember in Keychain**; and
- **Forget Discord session**.

Deletion review shows exact selected count, already-complete count, estimated minimum duration, pacing note and the unsupported-integration warning. During execution, selected messages become pending immediately, duplicate submission is disabled, and the job card shows deleted, absent, skipped, failed and uncertain counts plus pause/resume/cancel.

## 8. Security and test gate

All behavior is developed test-first with synthetic Discord IDs/tokens and local HTTP/WebSocket fixtures. Automated tests never contact Discord, launch a real browser, inspect an existing profile or use a private archive. Real destructive testing requires the user's separate explicit authorization and a disposable Discord account/message set.

Required acceptance coverage:

- manual token normalization, bounds, masking and no token echo;
- exact `/users/@me` identity match and mismatch rejection;
- vault migration, opt-in persistence, forget, denial caching and zeroization-sensitive ownership;
- browser detection without profile reads; Chromium and Firefox protocol fixture capture; origin/header filtering; cancellation, timeout, child termination and temporary cleanup;
- exact archive locator planning and account/source isolation;
- every HTTP outcome, rate-limit scope, bounded retry, cancellation and ambiguous outcome;
- durable progress and restart behavior without message content or tokens;
- source selection/import progress and archive-only first run;
- frontend contract rejection, stale-context protection and accessible dialog/job behavior;
- production bundle checks, provider-boundary checks, dependency/license review, strict Rust/TypeScript lint/build, and Docker checks on both architectures.

The README must plainly disclose the Discord account risk, explain both sign-in methods, state that automatic capture uses an isolated temporary profile, and explain how to forget the session.
