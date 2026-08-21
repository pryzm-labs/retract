# Chat History Cleaner — Product and Engineering Plan

Status: open-source preview candidate on 2026-08-20. Production onboarding requires a real Telegram connection; synthetic fixtures are restricted to automated tests and documentation screenshots. A pinned Apple-silicon TDLib 1.8.64 artifact is bundled by the build. Destructive release remains gated on the TDLib test-DC matrix in `docs/TEST_PLAN.md`.

## 1. Product direction

Build a local-first desktop utility that lets a person sign into their own Telegram account, find messages across chats, preview exactly what Telegram will allow them to remove, and execute `Delete for everyone` safely.

The product should optimize for three jobs:

1. Find and selectively delete messages by words, chat, sender, date, and media type.
2. Clear all history in a DM or group when Telegram reports that the current account can do so.
3. Explicitly delete history only for the current account and remove an abandoned chat from its chat list.
4. Permanently delete a group itself when Telegram reports that the current account can do so.

The key product rule is: never infer destructive authority from the label "admin." Telegram exposes separate ownership, admin rights, per-message delete rights, whole-history delete capability, and whole-chat delete capability. The app must display and obey the server-reported capability for the exact action. When leaving a group or channel, Retract favors the broadest confirmed history-cleanup capability—whole-history revoke, then enumerated per-message admin deletion, then the current account’s own messages—before removing membership. Permanent group dissolution is always a separate owner-only action.

Product name: **Retract** — a direct description of taking messages back without implying that external copies can be erased. Do not include "Telegram" in the product name or use the Telegram logo; Telegram's API terms restrict both.

## 2. What is actually possible

This must be a Telegram **user client** using MTProto through TDLib. The Bot API cannot enumerate a user's entire account history and is not appropriate for this product.

| User intent | TDLib operation | Important constraint |
| --- | --- | --- |
| Delete selected messages | `deleteMessages(chat_id, ids, revoke: true)` | Re-check `messageProperties.can_be_deleted_for_all_users` for every message immediately before deletion. |
| Revoke an entire history and remove the chat | `deleteChatHistory(chat_id, remove_from_chat_list: true, revoke: true)` | Only enable when `chat.can_be_deleted_for_all_users` is true. This removes the chat from the current account’s list; it does not necessarily dissolve a group, and a later DM can recreate the conversation. |
| Remove history and the chat only for this account | `deleteChatHistory(chat_id, remove_from_chat_list: true, revoke: false)` | This is a distinct self-only action, never a fallback. Only enable when `chat.can_be_deleted_only_for_self` is true; peers retain their copies and a future message can recreate the chat. |
| Permanently delete a group | `deleteChat(chat_id)` | Only enable when `chat.can_be_deleted_for_all_users` is true. This removes members and releases group usernames. Treat this as the true "nuke group" action. In practice this is generally owner-level authority, not merely any admin. |
| Delete every message from one participant | `deleteChatMessagesBySender` | Supergroups only; requires the `can_delete_messages` admin right. |
| Delete by a date range | `deleteChatMessagesByDate` where supported; otherwise search then delete IDs | The direct method is only for private chats and basic groups, and does not delete messages sent in the last 30 seconds. Other chat types need a paged search/delete job. |
| Search across normal chats | `searchMessages` | Includes Main and Archive, excludes secret chats, pages up to 100 results at a time. |
| Search secret chats | `searchSecretMessages` | Separate operation over secret messages available to this TDLib session/device. |

Deleting by message ID is content-agnostic, so text, photos, videos, files, voice notes, video notes, audio, GIFs/animations, stickers, polls, locations, contacts, link previews, and other media do not need separate deletion implementations. Media type still matters for filtering, preview, thumbnail rendering, and album-aware selection. Service messages and unusual content must remain subject to their per-message capability flags.

Telegram also documents a server-side size restriction for clearing a supergroup history: `channels.deleteHistory` can fail with `CHANNEL_TOO_BIG` above 1,000 participants. The app should not promise that all admins can clear every group; it should show the capability, attempt the supported operation, and explain Telegram's returned error without falling back to a misleading "success."

Deletion is irreversible. There is no app-level trash or undo once Telegram accepts the operation.

## 3. MVP scope

### Include

- QR-code login first, with phone/code/2FA fallback.
- Main, Archive, DMs, basic groups, supergroups, channels, and locally available secret chats.
- Chat browser with highly visible ownership/admin/capability badges.
- Global keyword search plus filters for chat, chat type, sender, date range, direction (`mine`, `others`, `any`), and content/media type.
- Per-chat history browsing for cleanup without a keyword.
- Message preview with text/caption, sender, timestamp, media type, album grouping, and deletion reach.
- Select messages, result pages, conversations, dates, senders, or all matching results.
- A dry-run summary before every deletion.
- Selected-message deletion, whole-history clearing, delete-by-sender, and true group deletion where allowed.
- Rate-limit-aware, resumable deletion jobs with cancellation between batches.
- A local, privacy-minimized job report showing deleted, skipped, failed, and still-visible counts.
- Light/dark mode, VoiceOver labels, keyboard navigation, and native-feeling macOS shortcuts.

### Exclude from the first release

- Sending, editing, forwarding, or reacting to messages.
- Mobile clients.
- Cloud accounts, analytics, telemetry, or a hosted backend.
- Fully unattended scheduled deletion.
- Regex/fuzzy search that requires downloading and permanently indexing all message bodies.
- Recovery or backup claims.
- Story deletion, profile cleanup, contact cleanup, and account deletion.

## 4. Information architecture and UI

Use a three-pane desktop layout:

```text
┌────────────────────┬──────────────────────────────────┬──────────────────────────┐
│ Chats              │ Search / Results                 │ Selection & impact       │
│                    │                                  │                          │
│ Search chats       │ "project codename"        [⌘F]  │ 142 selected             │
│ ◉ Owner            │ [DM][Groups][Mine][Media][Date]  │ 136 delete for everyone  │
│ ◆ Can delete       │                                  │   4 self-only             │
│ ◇ Admin limited    │ ☑ Alice · text · Aug 12          │   2 cannot delete         │
│ ○ Member           │ ☑ Design Team · photo caption    │                          │
│                    │ ☐ Family · voice note            │ [Review deletion…]       │
│ Main / Archive     │                                  │                          │
└────────────────────┴──────────────────────────────────┴──────────────────────────┘
```

### Capability badges

Avoid a single ambiguous "Admin" highlight. Use both role and effective cleanup power:

- **Owner** — crown badge; show "Can delete group" only if the chat capability flag is true.
- **Admin · delete messages** — filled shield; `can_delete_messages` is true.
- **Admin · limited** — outlined shield; admin, but cannot delete other people's messages.
- **Member** — no shield.
- **Clearable for everyone** — separate red broom indicator when whole-history deletion is currently supported.

Hover/click help should say *why* an action is or is not available. Keep color redundant with icon and text for accessibility.

### Main workflows

#### A. Search and selectively clean

1. Enter a word or phrase in global search.
2. Refine by chat/chat type, sender, date, ownership, and media type.
3. Inspect results grouped by chat or chronologically.
4. Select individual messages, albums, a page, or all matches.
5. Run capability analysis.
6. Show an impact summary: "delete for everyone," "only for you," and "cannot delete."
7. Default to deleting only the `for everyone` set. Never silently downgrade to `only for me`.
8. Confirm, run, and verify through TDLib updates/re-queries.

#### B. Clean a date range in one chat

1. Open a chat and choose **Clean range**.
2. Choose start/end dates and optional sender/media filters.
3. Enumerate matches and show counts before enabling deletion.
4. Use the direct date method only where supported; otherwise build a stable list of message IDs and process it in batches.

#### C. Clear history

1. Open **Chat actions → Clear history**.
2. Explain whether the result affects everyone or only the current account.
3. Offer "also remove from my chat list" separately.
4. Require a checkbox acknowledging irreversibility and a final hold-to-confirm or reauthentication step.

#### D. Delete the group itself

1. Only show this in **Danger Zone** when `chat.can_be_deleted_for_all_users` is true.
2. Explain: all messages removed, all members removed, usernames released, no undo.
3. Display title, public usernames, approximate member count, and current role.
4. Require typing the exact group title and local biometric/password confirmation.
5. Call `deleteChat`, wait for success/update, then show a permanent result record with no message content.

## 5. Search behavior

Start with TDLib's own search rather than building a second full-text corpus.

- Global normal-chat search: `searchMessages` with opaque pagination offsets.
- Per-chat search: `searchChatMessages`, which supports sender/topic/filter criteria where Telegram supports that combination.
- Secret search: `searchSecretMessages` in a distinct local-only result section.
- Empty keyword: per-chat history pagination or media filters, not global `searchMessages` abuse.
- Privacy scan: locally inspect paginated per-chat history for structured sensitive-data patterns, annotate in-memory results with categories, and disclose false-positive and no-OCR limitations.
- Search captions as returned by Telegram; present matched text and media uniformly.
- State clearly that "all chats" means all accessible Main and Archive chats plus secret-chat data available to this TDLib session. It cannot find already deleted, expired, inaccessible, or never-synchronized content.

For a later "advanced local search" feature, make indexing opt-in and encrypted. It can add exact phrase, regular expression, case sensitivity, saved queries, and offline search. Do not make it a prerequisite for the MVP.

## 6. Safe deletion engine

Model deletion as durable jobs rather than UI button callbacks.

```text
Draft → Enumerating → Capability check → Awaiting confirmation
      → Running → Rate-limited/Paused → Verifying → Completed/Partial/Failed
```

### Job rules

- Freeze the candidate set as `(account_id, chat_id, message_id)` records before confirmation.
- Store only the minimum metadata needed to resume: IDs, operation, state, attempts, and error category. Avoid storing message bodies or thumbnails in the job database.
- Immediately before each batch, re-fetch message properties and chat/admin state. Rights can change between preview and execution.
- Partition candidates into `delete_for_everyone`, `self_only`, `not_deletable`, and `missing`.
- Do not mix chat IDs in a delete call.
- Process each chat sequentially with a bounded batch size established by integration tests; allow limited concurrency across chats only after rate-limit testing.
- On Telegram flood/rate errors, persist the cursor, show the required wait, and resume only after the indicated interval.
- Cancellation takes effect between requests; an in-flight accepted deletion cannot be undone.
- Treat repeated deletion of already-missing messages as idempotent completion where Telegram semantics allow it.
- Verify using TDLib delete updates and sampled re-fetches. Report partial completion honestly.
- Keep self-only deletion as a separate explicit action; never use it as an automatic fallback from "for everyone."

### Confirmation tiers

| Risk | Example | Confirmation |
| --- | --- | --- |
| Low | 1–10 selected messages | One impact dialog. |
| Medium | More than 10 messages, all search matches, date range | Review screen plus irreversible checkbox. |
| High | Clear whole history | Review, typed chat title or hold-to-confirm, and optional system authentication. |
| Critical | Delete group | Typed exact title plus system authentication; no keyboard shortcut. |

Do not add an artificial countdown that makes the app tedious. Make the impact legible and make the critical confirmation hard to trigger accidentally.

## 7. Recommended architecture

### Stack

- **Desktop shell:** Tauri 2.
- **UI:** React + TypeScript + Vite, with a virtualized result list and an accessible component system styled specifically for macOS.
- **Core:** Rust in the Tauri core process.
- **Telegram integration:** Official TDLib, pinned to an exact version, linked through its `tdjson` C/JSON interface.
- **App state:** Rust domain services as the source of truth; typed events/view models over a narrow Tauri IPC allowlist.
- **Job persistence:** a small encrypted local database or authenticated encrypted records containing no message content.
- **Secrets:** a random TDLib database encryption key in macOS Keychain; platform credential stores on Windows/Linux. Never expose the API hash, auth/session state, encryption key, or raw TDLib interface to JavaScript.
- **Testing:** Rust unit/property tests, TypeScript component tests, Playwright UI tests, and TDLib integration tests against Telegram's test DC before production-account testing.

### Why this stack

TDLib handles MTProto networking, login state, encryption, ordered updates, local data storage, and cross-platform behavior. Its maintainers recommend the JSON interface for most foreign-language integrations. Tauri provides a small cross-platform desktop shell and a useful trust boundary: the WebView renders data, while the Rust core owns credentials, capabilities, storage, and destructive calls.

This is preferable to:

- **A Bot API app:** technically incapable of the account-wide job.
- **Browser automation:** fragile, incomplete, difficult to verify, and unsafe for large destructive operations.
- **A macOS-only SwiftUI app:** excellent native UI, but it creates a second UI implementation if Windows/Linux follow. Choose it only if native macOS polish is more important than cross-platform delivery.
- **Electron:** viable, but it bundles a larger runtime and places more pressure on Node/native-binding hardening. It offers no Telegram-specific advantage over Tauri here.

### Component boundaries

```text
React UI
  └─ typed commands/events only
Tauri command boundary
  ├─ AuthService
  ├─ ChatCatalog + CapabilityService
  ├─ SearchService
  ├─ DeletionPlanner
  ├─ DeletionExecutor + RateLimitPolicy
  ├─ JobStore + AuditSummary
  └─ TDLibAdapter
       └─ pinned native tdjson library → Telegram
```

The frontend should receive purpose-built DTOs such as `ChatCapabilityView` and `DeletionPlanView`, not raw TDLib JSON. The backend must independently validate every UI command.

### Suggested repository layout

```text
apps/desktop/                  React/Vite UI
src-tauri/                     Tauri/Rust entrypoint and IPC
crates/telegram-adapter/       Safe TDLib wrapper
crates/cleaner-domain/         Capability and deletion planning rules
crates/job-store/              Encrypted resumable job state
vendor/tdlib/                  Pinned source/submodule or reproducible fetch metadata
tests/fixtures/                Sanitized TDLib response fixtures
docs/                          Threat model, release and test plans
```

## 8. Security and privacy requirements

This application is unusually sensitive because it can read private history and irreversibly delete shared data.

- Local-only by default; no backend is needed.
- Use TDLib's message database encryption with a random key kept in the OS credential store.
- Keep the TDLib database and downloaded files in app-specific directories with restrictive permissions.
- Do not download full media merely to show search results; request thumbnails only when needed and make cache clearing visible.
- Auto-lock the app after inactivity and support immediate **Lock** and **Sign out and erase local data** actions.
- Redact message content, phone numbers, auth codes, API hash, file paths, and encryption keys from logs and crash reports.
- Disable telemetry in MVP. If ever added, make it opt-in and prohibit message/chat/sender content.
- Use a strict Content Security Policy, no remote scripts, a minimal Tauri capability allowlist, and no general shell/filesystem command exposed to the WebView.
- Validate IPC payload sizes, chat/message IDs, account ownership, and operation type in Rust.
- Preview archives must be reproducible, manifest-verified, checksummed, hardened, and ad-hoc signed. A future official distribution should add Apple Developer signing, notarization, and signed updates.
- Publish a threat model covering malicious message content, compromised WebView dependencies, local database theft, forged IPC, update compromise, and accidental mass deletion.
- Never use Telegram-derived data for AI/ML training; Telegram's terms explicitly prohibit it.

## 9. API, compliance, and distribution risks

Before a public release:

1. The source-first preview asks users to supply their own `api_id` and `api_hash` through the local UI. Before broad consumer distribution, review Telegram policy and whether Pryzm Labs should provide product credentials instead.
2. Prominently disclose in onboarding and store copy that this is an independent third-party app using the Telegram API.
3. Do not use "Telegram" in the name unless prefixed by "Unofficial," and do not use the official logo.
4. Review the requirement that third-party clients implement Telegram's basic functionality correctly. A deletion-only utility may be considered a specialized client, so obtain written guidance from Telegram before broad distribution.
5. Ensure every destructive action is visible, specifically consented to, and scoped by the user. Telegram prohibits acting without the user's knowledge and consent.
6. If channel content is displayed, evaluate Telegram's sponsored-message requirement for third-party clients.
7. Test authorization and destructive flows on Telegram's test DC first. Production client logins are monitored and abuse/flooding can lead to account or API restrictions.

For the open-source preview, publish source plus a clearly labeled unsigned/unnotarized Apple-silicon `.app.zip` with checksum and provenance. Users can build locally or use macOS's per-app **Open Anyway** path. App sandboxing and native-library packaging should be validated before any future Mac App Store release. Windows and Linux can follow from the same Tauri/TDLib core, but native TDLib builds, credential stores, packaging, signing, and end-to-end tests are still per-platform work—not a free checkbox.

## 10. Additional features worth adding

### High-value, low-to-medium complexity

- **Admin cleanup dashboard:** every owned/administered chat, exact delete permission, member count, oldest indexed message, and last cleanup.
- **Delete by sender:** especially useful for spam cleanup in supergroups.
- **Safety exclusions:** never delete pinned messages, selected chats, selected senders, recent messages, or messages matching protected terms.
- **Saved cleanup recipes:** filters only; require a fresh preview and confirmation for each run.
- **Album-aware selection:** selecting one album item offers "this item" or "entire album."
- **Storage cleanup:** separately clear local downloaded media/cache without deleting server messages.
- **Privacy health view:** counts by chat, age bucket, and media type without uploading or retaining message bodies.
- **Export plan:** export a manifest of IDs/dates/types before deletion. Keep message text/media excluded by default; if content export is ever supported, make it explicitly encrypted.
- **Post-run verification:** recheck representative IDs and provide a clear partial-failure report.

### Later, after safety and compliance validation

- Opt-in encrypted local advanced search with regex and exact phrases.
- Reviewed retention reminders such as "show me messages older than two years every month." Prefer a review queue over unattended deletion.
- Multiple Telegram accounts with completely separate TDLib directories and encryption keys.
- Forum topic and channel direct-message cleanup views.
- Admin-log correlation for moderation teams, without claiming it restores deleted content.
- Windows and Linux releases.

Avoid features that create hidden or automatic behavior: silent background deletion, default schedules, growth analytics over message contents, AI classification of histories, or cloud synchronization of the local index.

## 11. Delivery phases

### Phase 0 — feasibility and compliance spike (1–2 weeks)

- Obtain app API credentials and clarify public-distribution compliance.
- Build pinned universal macOS TDLib artifacts (`arm64` and `x86_64`).
- Prove QR/phone/2FA login, encrypted database startup, chat enumeration, global search, capability reads, single-message revoke, and group/history deletion on the test DC.
- Record real server errors and update behavior as fixtures.
- Exit criterion: every destructive promise in the UI maps to a tested TDLib capability and operation.

### Phase 1 — read-only explorer (2–3 weeks)

- Scaffold Tauri/React/Rust architecture.
- Implement auth, chat list, role/capability badges, global/per-chat/secret search, filters, message preview, and virtualized pagination.
- Add lock/logout/local-data-erasure controls.
- Exit criterion: users can find and select targets, but no delete command is enabled in production.

### Phase 2 — selective deletion MVP (2–3 weeks)

- Implement deletion planning, per-message capability checks, confirmations, batched execution, rate-limit handling, cancellation, persistence, and verification.
- Add text/media/album cases and partial-failure reporting.
- Exit criterion: reliable selected-message deletion across DMs, groups, supergroups, channels, and secret chats where permissions allow.

### Phase 3 — bulk and admin tools (2–3 weeks)

- Add date-range workflows, delete-by-sender, whole-history clearing, true group deletion, and the admin dashboard.
- Add typed/system-auth confirmations and permission-change tests.
- Exit criterion: critical actions are gated by live capabilities and survive interruption without duplicate or expanded scope.

### Phase 4 — hardening and macOS release (2–4 weeks)

- Threat model, dependency audit, fuzz IPC parsing, performance testing on large accounts, accessibility QA, signing, notarization, signed updates, privacy policy, and support/error documentation.
- Exit criterion: signed beta with a rollback path for the app itself, honest known limitations, and no claim that server deletions can be recovered.

### Phase 5 — cross-platform (about 1–2 weeks per platform after core stability)

- Build/package TDLib, implement credential-store adapter, validate WebView behavior, add installer/signing pipeline, and rerun the destructive integration suite on Windows and Linux.

## 12. Testing strategy

- **Domain unit tests:** capability matrices, plan partitioning, confirmation tiers, no self-only downgrade, and idempotency.
- **Property tests:** a deletion plan never expands beyond the frozen user selection; skipped/failed/deleted partitions remain disjoint and exhaustive.
- **Adapter contract tests:** sanitized recorded TDLib JSON for every chat/member/message status and error class.
- **Test DC integration:** login states, paged search, all content types, album deletion, permission loss mid-job, group size errors, rate limits, disconnects, app crash/restart, and duplicated updates.
- **UI tests:** selection semantics, filter counts, badge accessibility, critical confirmation, cancellation, and partial-result rendering.
- **Security tests:** hostile message markup/text, oversized results, malformed TDLib events, IPC invocation from untrusted WebView content, log redaction, local database theft, and update signature failure.
- **Manual destructive matrix:** DM × basic group × supergroup × channel × secret chat, crossed with owner/admin-with-delete/admin-without-delete/member and mine/other/service/media messages.

## 13. Product success criteria

- A user can find a known word across Main and Archive and understand secret-chat coverage.
- Every result accurately says whether deletion affects everyone, only the current user, or nobody.
- No action ever deletes more message IDs or chats than the reviewed plan.
- A stopped/crashed/rate-limited job resumes from durable state and reports exact partial completion.
- The app never silently converts a failed `for everyone` request into `for me`.
- Owner/admin privileges are obvious without implying unsupported authority.
- Whole-group deletion is impossible to trigger through a normal delete shortcut.
- No message bodies, media, auth secrets, or phone numbers leave the machine in the default configuration.

## 14. Primary references

- [Telegram API Terms of Service](https://core.telegram.org/api/terms)
- [Creating a Telegram Application](https://core.telegram.org/api/obtaining_api_id)
- [TDLib project and JSON interface guidance](https://github.com/tdlib/td)
- [TDLib `searchMessages`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1search_messages.html)
- [TDLib `searchSecretMessages`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1search_secret_messages.html)
- [TDLib `messageProperties`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1message_properties.html)
- [TDLib `deleteMessages`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1delete_messages.html)
- [TDLib `deleteChatHistory`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1delete_chat_history.html)
- [TDLib `deleteChat`](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1delete_chat.html)
- [TDLib administrator rights](https://core.telegram.org/tdlib/docs/classtd_1_1td__api_1_1chat_administrator_rights.html)
- [Telegram `channels.deleteHistory`](https://core.telegram.org/method/channels.deleteHistory)
- [Tauri process model](https://v2.tauri.app/concept/process-model/)
- [Apple Keychain Services](https://developer.apple.com/documentation/Security/keychain-services)
