# Destructive integration and release gate

## Rule

No production Telegram account may be used until every applicable test below passes against Telegram’s test data center with disposable accounts and groups. Record the Retract commit, TDLib source commit and SHA-256, macOS version, architecture, account role, request, TDLib result, and observed postcondition for each case.

Set `RETRACT_TELEGRAM_TEST_DC=1`. Confirm the UI reports the intended Telegram account and TDLib 1.8.64; an unconfigured setup state is not a passing live test.

## Automated gate

Run from a clean checkout:

```sh
npm ci
npm run check
cargo clippy --manifest-path crates/cleaner-domain/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm audit --audit-level=high
npm run tauri build -- --debug
```

Required: no failed tests, warnings treated as errors, no high/critical npm advisories, successful app bundle, and no unexpected network requests other than Telegram endpoints.

## Test identities

Create disposable test-DC users covering: DM peer, basic-group owner, supergroup owner, admin with delete rights, limited admin, ordinary member, and a sender whose messages can be moderated. Include one secret chat on the same local TDLib session if supported by the test setup.

## Search and catalog matrix

- Main and Archive enumerate without duplicate chat IDs.
- Global keyword search pages beyond 100 results and deduplicates results.
- Empty-query per-chat browse works.
- A keyword with **All chats** selected returns matching messages from normal and secret chats.
- Privacy scan finds each supported category in deterministic fixtures and labels the category without writing matched values to job state.
- Privacy scan respects chat, sender, date, content-kind, and pinned-message filters.
- Deep account scans paginate past 100 messages per chat, deduplicate page boundaries, and include secret chats.
- The UI states that privacy detection is heuristic and does not OCR image pixels or inspect external copies.
- **No reply sent** includes a normal DM/group only when an account-sender search finds no outgoing message; ambiguous admin and secret-chat cases are excluded.
- **Empty** includes a chat only after a non-local `getChatHistory` probe returns no messages.
- Secret-chat search uses the separate secret search path and never claims remote history the device lacks.
- Direction, date, pinned, chat scope, admin scope, archive, and every content-type filter are verified.
- Content cases: text, photo caption, album, video, document, voice, audio, animation/GIF, sticker, poll, location, contact, and service message.
- Album selection is atomic and hidden selections remain visible in the impact summary.
- Owner, full admin, limited admin, and member badges match Telegram’s actual rights.

## Destructive matrix

For every accepted operation, verify from both participating accounts after TDLib updates settle.

| Case | Expected result |
| --- | --- |
| Own DM message, everyone-capable | Removed for both users; job counts one deleted |
| Message reported self-only | Skipped; no delete request; remains for peer |
| Protected/nondeletable message | Skipped and explained |
| Mixed selection across chats/media | Only everyone-capable frozen IDs removed; no adjacent message affected |
| Album | All selected album members handled; each ID rechecked |
| More than 100 selected messages | Multiple batches, each at most 100, correct durable cursor |
| Permission revoked after preview | Action fails/skips closed; no self-only fallback |
| Message removed by another client after preview | Conservative skipped/partial result, no unrelated deletion |
| Clear DM/basic-group history where capable | History removed for everyone; chat existence matches Telegram semantics |
| Remove empty or incoming-only DM for self | Chat history/list entry disappears only for the signed-in account; peer copy is unchanged |
| Self-only removal capability revoked after preview | Action fails closed; no revoke-for-everyone or leave request runs |
| Leave as whole-history-capable admin | Complete history revoke runs while membership is intact, then leave/removal runs; permanent group deletion is not invoked |
| Leave as admin with per-message delete authority | Every enumerated participant message is frozen and rechecked; all eligible IDs are deleted before leave/removal; protected IDs are reported |
| Leave group/channel with revocable outgoing messages | Frozen outgoing IDs are rechecked and deleted for everyone first; leave/removal runs afterward; rejected IDs are reported without a self-only fallback |
| Leave group/channel with no revocable outgoing messages | Confirmation says there is nothing to revoke; membership and the local chat copy are still removed when Telegram permits |
| Clear supergroup history | Capability respected; `CHANNEL_TOO_BIG` or other Telegram errors shown honestly |
| Delete by sender as authorized admin | Only that immutable sender’s messages in that chat disappear |
| Delete by sender as limited admin/member | Plan creation or action blocked |
| Permanently delete owned group | Exact group removed, members removed, public username behavior observed |
| Delete group as non-owner/admin | Action unavailable or rejected; no alternative operation runs |

## Confirmation and abuse cases

- Alter plan ID, fingerprint, chat ID, message ID, sender ID, operation, or typed title through IPC; every mutation must fail.
- Reuse a high-impact system-auth grant against another plan; fail.
- Wait more than 60 seconds after macOS authentication; fail.
- Reuse the same grant or execute the same plan twice; fail.
- Cancel Touch ID/password; fail without creating a job.
- Complete owner authentication but change the title proof; fail without consuming unrelated authority.
- Close the dialog, use Escape, switch chats, and change filters; no deletion occurs.
- Verify low/medium selected-message cleanup does not accidentally request group-wide authority.

## Reliability cases

- Inject `FLOOD_WAIT_2`/429 for capability fetch and deletion. Job becomes queued, stays cancellable, persists its wait, rechecks permissions, and resumes.
- Cancel while queued for flood wait. Job becomes cancelled and makes no later call.
- Kill the app before a frozen-ID batch, during a completed batch response, and between batches. Restart resumes only the frozen IDs and never deletes outside the reviewed plan.
- Kill the app around whole-history, self-only history, sender-wide, and permanent group-deletion calls. An ambiguous nonterminal job must stop with `restart_requires_new_review`; it must not replay against later messages until the user creates and authorizes a new plan.
- Copy an authenticated test-DC job store into the production profile. Startup must reject it as profile-bound ciphertext and execute nothing. A legacy unbound nonterminal store must be retained only as a stopped job requiring new review.
- Disconnect/reconnect network during search and deletion; errors remain explicit and UI stays responsive.
- Corrupt the encrypted job file; startup fails closed with no execution.
- Change/remove the Keychain key; encrypted state cannot be decrypted and no plan auto-runs.
- Load a TDLib version other than 1.8.64; authorization stays blocked with an explicit error.

## macOS and accessibility

- Apple-silicon builds on macOS 12 and the current supported macOS release.
- Light/dark mode, reduced motion, 200% effective zoom, keyboard-only flow, visible focus, and VoiceOver labels/order.
- For low, medium, high, and critical operations: Touch ID available, Touch ID unavailable with password fallback, cancelled password, locked-out biometrics, and app background/system-cancel cases. Confirm the native reason identifies the immutable target and plan token.
- Code signing, hardened runtime, notarization, Gatekeeper launch, update path, Keychain ACL prompts, and uninstall/reinstall behavior.

## Production exit criteria

- Two-person review of evidence for every destructive and confirmation case.
- No unresolved critical/high security findings.
- TDLib binary digest matches the reviewed artifact and is bundled/signed with the app.
- Privacy copy and support documentation state the residual-copy limitations.
- A rollback build exists. Rollback can stop future deletions but cannot restore Telegram-accepted deletions.
