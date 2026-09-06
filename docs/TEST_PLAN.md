# Destructive integration and release gate

## Rule

No production Telegram account may be used until every applicable test below passes against Telegram’s test data center with disposable accounts and groups. Record the Retract commit, TDLib source commit and SHA-256, macOS version, architecture, account role, request, TDLib result, and observed postcondition for each case.

Use the **Use Telegram's test server** setting (or the developer override `RETRACT_TELEGRAM_TEST_DC=1`). Confirm account verification completes and the UI reports the intended Telegram account and TDLib 1.8.64; auth-ready or an unconfigured setup state is not a passing live test. Live deletion and credential/Keychain checks require explicit authorization for those disposable identities; the automated gate below does not access them.

## Automated gate

The preferred gate uses pinned Docker toolchains, locked dependencies, shared named caches, cache-only output, and non-root/offline project execution:

```sh
npm run container:check
docker buildx build --platform linux/amd64 --target checks --output type=cacheonly --progress plain .
docker buildx build --platform linux/arm64 --target checks --output type=cacheonly --progress plain .
```

These commands run unfiltered Vitest, release/public metadata tests, TypeScript/production build, fixture exclusion, provider-boundary controlled tests plus real-tree lint, and Rust tests/formatting/Clippy for `cleaner-domain`, `retract-domain` and `src-tauri`. Require no failures or unexpected ignores and treat Clippy warnings as errors. The existing explicitly ignored 100,000-item archive corpus resource gate remains opt-in and is not a failure by itself. Do not export runnable images or prune Docker state for verification. The npm wrapper runs on the host only to dispatch Docker; project tests/builds run inside the container.

The native `native-macos-package` job in [secure-build.yml](../.github/workflows/secure-build.yml) separately runs `npm ci --ignore-scripts --no-audit --no-fund` and `npm run package:unsigned` on macOS. It verifies the app, bundled TDLib, ad-hoc signing, checksum and manifest. Linux container success does not establish macOS packaging, native authentication, launch or accessibility success. If that job cannot be run for the tested revision, report it as unavailable/not run; do not fabricate a package or access real Keychain/session data for automated proof.

An authorized native developer may additionally run `npm run check` to exercise the opt-in bundled TDLib loading smoke test. A separately authorized online `npm audit --audit-level=high` checks current advisories; it is not part of the offline Docker result. Record these outcomes separately, including unexpected network access during live testing.

### Encrypted archive integration

The automated archive gate uses synthetic files and disposable injected keys only. Both macOS packaging workflows run `persistence::archive::` and `secure_store::vault` test filters; these include original codec tests, store/recovery/ingestion/query/removal tests, bounded-worker lifecycle tests, and real-file/process credential-lease tests with injected I/O. No real-Keychain or Telegram test is included in these filters. Normal packaging leaves the optional benchmark feature disabled.

Verify lazy construction, settings-first and archive-first credential access, fail-fast second-process contention without vault I/O, cancelled open/shutdown waiters, rejection after terminal clearing, two pending batches, cancellation while both slots are occupied, exact retry progress, wrong scopes, and surviving-source queries after removal. Use the opt-in [100,000-item benchmark](ARCHIVE_STORAGE.md#opt-in-synthetic-benchmark) once for a relevant implementation revision, recording cumulative OS peak RSS, exact phase DB/WAL lengths, sampled disk highs, elapsed phases and committed progress. It is not part of every test run.

Before enabling an archive importer, an authorized native operator must quit all older Retract copies, then verify final-identity Keychain prompts/ACLs, v1-to-v2 secret preservation, denied/locked Keychain behavior, competing app/profile ownership and complete shutdown drain. Verify that database/WAL/journal/temporary files and diagnostic logs contain no plaintext canaries, and that memory/disk-pressure compaction failures leave removal durable and maintenance visibly pending. Vault downgrades and forensic-erasure claims are unsupported. Record these manual results separately from synthetic CI and rerun the Apple-silicon test/package job on the final reviewed commit.

### Telegram provider-refactor characterization

The full Docker gate includes frozen v1 readers/native behavior, the immutable pre-extraction Telegram recipe/`RTRCT03` corpus, current v2 lifecycle tests, direct query/cleanup ownership checks and the provider architecture rule. For focused synthetic iteration on Linux `arm64`:

```sh
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=npm test -- src/provider-lifecycle.test.tsx src/providers/contract.test.ts src/ipc-contract.test.ts' .
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=npm run check:provider-boundaries' .
docker buildx build --platform linux/arm64 --target focused-checks --output type=cacheonly --progress plain --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path src-tauri/Cargo.toml providers::telegram::migration_tests && cargo test --offline --locked --manifest-path src-tauri/Cargo.toml providers::telegram::query_tests && cargo test --offline --locked --manifest-path src-tauri/Cargo.toml providers::telegram::remediation::tests && cargo test --offline --locked --manifest-path src-tauri/Cargo.toml foundation_lock_tests' .
```

These focused filters are not substitutes for the full gate. The frozen numeric v1 fixture remains unchanged historical evidence; actual v2 commands use scope, session generation and opaque refs without a numeric fallback. The separate migration corpus freezes all nine native recipes and exact authenticated v3 bytes from the pre-extraction implementation. See [TELEGRAM_CHARACTERIZATION.md](TELEGRAM_CHARACTERIZATION.md) for its provenance, the direct provider ownership inventory and preserved native behavior.

For reviewed migration commit `c7f2f5f29387a71c9ac4051b7dd66508baee1311`, both required Linux architecture gates passed on 2026-09-06: 158 frontend tests, 329 backend tests with one existing opt-in archive corpus test ignored, 17 cleaner-domain tests, 19 retract-domain tests, 7 release tests, 10 controlled boundary tests plus the real-tree check, production bundle/public repository verification, all formatting checks and strict Clippy. Both builds copied that exact committed tree and executed the project checks; the BuildKit results were `yafbi07vc0a8p6gv3hjryumwt` for `arm64` and `ado6le3q5vvow7wk1pojcyq9u` for `amd64`. No archive corpus benchmark was run because the provider migration does not alter archive indexing. Independent whole-branch review of `0d799b0..c7f2f5f` completed with no Critical or Important issues. The earlier runtime-only `3a78bd207d89be3d3bc93b8cfac6930ce04017e6` results remain useful historical evidence; do not mistake them for the current accepted head or transfer the full-gate claim to later changes.

This Linux evidence does not complete native macOS packaging, TDLib loading, Keychain/LocalAuthentication, rendered VoiceOver/short-viewport behavior, or live test-DC parity. Those gates remain not run until an exact reviewed revision is authorized and, for CI, published. Changes after the reviewed `c7f2f5f` baseline require proportionate covering checks and review; the immediate documentation-only closure uses the focused public-repository check and scoped re-review rather than claiming another full runtime gate.

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

- Alter contract version, provider/account/source, session generation, plan ID/fingerprint, resource ref/locator, ordered effect, recipe or typed title through v2 IPC; every unauthorized mutation must fail.
- Switch account or sign out while the native owner prompt is open or after approval; the stale grant must not authorize a new context.
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
- Kill the app before a frozen-ID batch, during a completed batch response, and between batches. Resume requires durable start authorization, supported frozen-idempotent work, a valid cursor and the same verified account/source. Ambiguous outcomes require new review and never become confirmed deletions.
- Kill the app around whole-history, self-only history, sender-wide, and permanent group-deletion calls. An ambiguous nonterminal job must stop with `restart_requires_new_review`; it must not replay against later messages until the user creates and authorizes a new plan.
- Copy an authenticated test-DC job store into the production profile. Startup must reject it as profile-bound ciphertext and execute nothing. A legacy unbound nonterminal store must be retained only as a stopped job requiring new review.
- Disconnect/reconnect network during search and deletion; errors remain explicit and UI stays responsive.
- Corrupt the encrypted job file; startup fails closed with no execution.
- Migrate each frozen legacy store: retain exact `jobs.pre-provider.enc`, stop unfinished history for new review and never assign it an account. Older readers reject active `RTRCT03`; the backup stays historical, never automatically restored or removed.
- Interrupt migration before/after replacement, present a conflicting backup or a missing active file with artifacts, and corrupt active v3 while retaining a valid backup. Startup must neither restore old work nor silently initialize empty history.
- Start a second process for the same profile: it must report profile already in use before reading its key. Same-process runtime replacement reuses the single store; the last owner releasing it permits reopening.
- Delay/fail/malform `getMe`, fail the durable identity save, rename/reconnect, and restart the same account. No usable context precedes verified durable mapping; existing mappings remain stable and production/test identities remain separate.
- Open otherwise resumable work under another verified account: it becomes blocked, keeps its cursor, schedules no mutation and still permits connection settings. Returning to the original account does not broaden its targets.
- Fail a required job/progress save: no candidate is published as durable and no later batch or membership/self-removal call proceeds.
- Change/remove the Keychain key; encrypted state cannot be decrypted and no plan auto-runs.
- Load a TDLib version other than 1.8.64; authorization stays blocked with an explicit error.

## macOS and accessibility

- Apple-silicon builds on macOS 12 and the current supported macOS release.
- Light/dark mode, reduced motion, 200% effective zoom, keyboard-only flow, visible focus, and VoiceOver labels/order. Exercise a large plan in a short viewport: every ordered effect, exact-title field, acknowledgement and final control must remain reachable. Existing jsdom CSS/DOM tests are not rendered viewport or native focus/VoiceOver evidence.
- Open full job details: selected/eligible/deleted/skipped/failed/uncertain counters stay distinct, retry countdown and blocked/new-review reasons are safe, legacy missing counts are labeled not recorded, and no raw provider text appears.
- For low, medium, high, and critical operations: Touch ID available, Touch ID unavailable with password fallback, cancelled password, locked-out biometrics, and app background/system-cancel cases. Confirm the native reason identifies the immutable target and plan token.
- Verify the unsigned preview's ad-hoc signature, hardened runtime, documented per-app Gatekeeper launch, update path, Keychain ACL prompts and uninstall/reinstall behavior. Apple Developer enrollment, paid signing and notarization are not preview prerequisites; preview manifests must truthfully state `notarized: false`.

## Production exit criteria

- Two-person review of evidence for every destructive and confirmation case.
- No unresolved critical/high security findings.
- TDLib binary digest matches the reviewed artifact and is bundled/ad-hoc-signed with the preview app.
- Privacy copy and support documentation state the residual-copy limitations.
- Preserve the prior source/build for diagnosis, but do not automatically downgrade or restore job state: an older binary cannot read active v3. The retained migration backup does not restore Telegram-accepted deletions and must not be replayed as a rollback procedure.
