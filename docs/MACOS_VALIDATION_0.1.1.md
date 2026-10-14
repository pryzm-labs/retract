# macOS 0.1.1 validation record

Recorded on 2026-10-14. This is evidence for the unsigned preview, not a claim that every manual acceptance case passed. No live messages were deleted during this validation.

## Owner disposition and unsigned installation/update check

On 2026-10-14 the owner explicitly instructed that manual testing be accepted as complete, based on their extensive previous tests, and authorized the unsigned installation/update check. Manual provider/usability coverage is therefore accepted by the release owner; exact earlier revisions and a per-case matrix are not independently established. No further live cleanup was performed.

The exact merged-main candidate was rebuilt from `3e0ebcb74ada1eb7c16aa075644ebb821eb751e6`; ZIP SHA-256 is `4c5f03c11c0aafc3c5ee90bc2fc2d6b25cea94d2355f0c0d3c12d5c449e049d7`, and executable CDHash is `2a1abfb3ec5822b4532450983bf20d200b9c1d87`. Pinned-toolchain packaging, production exclusion, strict signature/contents/dependency checks and checksum verification passed; the disposable build tree was removed. Main's [Secure build run 38592158392](https://github.com/pryzm-labs/retract/actions/runs/38592158392) passed both Linux architectures and native macOS tests/unsigned packaging.

There is no public 0.1.1 artifact yet. The install check therefore uses real Brave browser downloads from a temporary loopback-only server serving only the verified package sets, not a simulated quarantine attribute. Baseline (`cb85f6d`) and candidate downloads matched their verified ZIPs byte-for-byte and both acquired browser-added quarantine. Finder's Archive Utility extraction preserved the baseline quarantine; Finder copied it into Applications, and the installed app retained quarantine, a valid ad-hoc signature and the expected baseline CDHash. No prior Applications installation was overwritten. The loopback server was stopped after downloading both packages.

The baseline launched without a new Gatekeeper block because its build identity had already been opened on this Mac; this is **not** a fresh-identity Open Anyway pass. The owner was asked to handle Keychain prompts, with baseline workspace verification and update replacement still pending. The rebuilt candidate has a distinct CDHash and is reserved for the update/trust check. No quarantine was stripped, no Gatekeeper policy was disabled, and no saved profile or credential was reset. Final GitHub-download/provenance verification remains a post-publication check, and macOS 12 execution remains unavailable on this host.

## Cold-start read reconciliation

On the next same-build reopen, the owner completed Keychain prompts and left the app untouched. Native accessibility inspection captured **Workspace not ready** / **The connection has changed. Review this action again.** before Retry. This is the fixed safe `stale_context` diagnostic, not a credential or saved-history reset request.

The startup path reads bootstrap and settings separately. Asynchronous identity verification may finish during bootstrap's final context recheck or between those reads. Strict backend checks correctly reject the stale request; the frontend previously treated it as a terminal startup failure. Synthetic tests reproduced both races through the real App, desktop adapter and decoders (three expected failures, eleven passes before the change).

The narrow frontend fix retries only these read-only startup checks, at most three attempts, and publishes bootstrap/settings together. Other errors retain explicit recovery. Unmounted and superseded StrictMode attempts cannot publish or retry. No backend, mutation, credential, persistence or profile-reset path changed. The initial sixteen focused connection-recovery tests passed in Linux arm64 Docker (BuildKit `29503g733wesc3g14y67wrond`); subsequent expanded tests, review and rebuilt native clean-start results are recorded below.

Independent read-only review found no critical or important issues. Its additional contextless-to-ready, catalog/search context and settings-error coverage is included in the expanded suite: 194 frontend tests pass. Docker run `8jigvrndcorqq6d1awdg3ngt9` also passed 505 backend tests (one existing opt-in ignore), domain/parser suites, production checks, release/public/boundary checks, formatting and strict Clippy. Its remaining benchmark-feature compile failed with Docker-internal `ENOSPC`, not a failing assertion; retry `voni7gt4odigrku7zqv5zucvm` hit the same disk limit. Those two attempts did not complete the gate. Only disposable Retract source/result layers and regenerable app-crate artifacts were reclaimed; dependency caches, other projects, saved profiles and credentials were preserved.

### Completed Docker gate and pre-merge review

On the owner's subsequent cleanup-and-merge request, exact cache entries were correlated with Retract build-history timestamps and Retract-owned source-layer ancestry. Scoped cleanup reclaimed **1.752 GB** of disposable source-context/result cache; compiler/dependency cache mounts, images, containers, volumes and other projects were excluded. These are regenerable build artifacts, not saved application data.

The complete Linux arm64 gate then passed at `c61af59` (BuildKit `kelzx8vwud7g03ohfhr2etqbm`): 194 frontend tests, 505 backend tests with one existing opt-in resource-test ignore, domain/parser suites, production/metadata/boundary checks, formatting, strict Clippy and all three benchmark-feature tests plus their strict Clippy check. Final independent whole-branch review found no critical or important issue; its minor stale-evidence wording was corrected. No code changed after this gate; subsequent documentation is checked separately. Native CI on the final PR revision and broader manual release acceptance remain distinct gates.

### Startup-fixed native candidate

- Package source: `cb85f6d5c3d701feec62c9425bb8bb6e186a5d7c`.
- ZIP SHA-256: `fb90efa1640a839b204a97f5b81a6f275dc6b1404c13a35946780fe6111c8f19`.
- Executable CDHash: `e19ae3dea2250cfc6b3a6ebf245ea660ecca9aa9`.
- Pinned-toolchain unsigned native packaging, production fixture exclusion, extracted-bundle allowlist, strict signature integrity, static TDLib/system dependencies, developer-path exclusion and checksum verification passed. The disposable native build tree was confirmed removed. The old candidate exited normally before the fresh candidate opened; no profile or credential reset was performed.
- The owner completed two Keychain prompts and reported **No error**. Before any Retry action, native accessibility independently confirmed the Telegram chat list and retained completed/legacy job views. This cold launch did not require Retry and the prior stale-context startup symptom was absent. A single successful launch does not establish every timing, locked/denied-Keychain or older-macOS case; broader manual acceptance remains open.
- The source chooser visibly showed **Loading imported accounts…**, then the retained Discord source became available without reimport. Opening it reached grouped navigation, archived results and preserved completed jobs. Connection settings showed **Verified for this archive · stored in Keychain** without token entry. Escape dismissed the dialog and left the usable workspace open. No credential edits, message selection, authorization or deletion occurred.

## Candidate identity

- Merged source: `aa6ccfbabd807251840d962d701824d1197b1c3c` (PR #50).
- Local package manifest source: `75453d6e9e4bd20aa06bc2d5e96a034049fb7d78`, the reviewed PR head. Both commits have the identical Git tree `0a620351fcc8a86ff1f04761cb9267a47e1d699c`; this was checked locally with `git rev-parse` and `git diff`.
- Artifact: `Retract-v0.1.1-macos-arm64.app.zip`.
- ZIP SHA-256: `8c135ff17345ef282b555d10911da862088dec7996b74b82009263b69c4c6d1d`.
- Bundle identifier: `app.retract.cleaner`; version/build: `0.1.1`.
- Executable CDHash: `ff70e1bf0bb520f522df8530bab3273497fd84bf`.
- Local host: Apple silicon, macOS 15.8.1 (24H32).

## Observed local checks

| Check | Result | Evidence/limits |
| --- | --- | --- |
| ZIP checksum | Pass | `shasum -a 256 -c` accepted the supplied checksum. |
| Extracted bundle allowlist | Pass | `scripts/verify-app-contents.sh` accepted the extracted app. |
| Signature integrity | Pass | `codesign --verify --deep --strict --verbose=2` reported valid on disk and satisfying its designated requirement. |
| Signature and runtime | Pass | `Signature=adhoc`, flags `0x10002(adhoc,runtime)`, no TeamIdentifier. No paid signing/notarization is implied. |
| Architecture and deployment target | Pass | Thin arm64 Mach-O; Info.plist and `LC_BUILD_VERSION` both specify macOS 12.0. This is not an execution test on macOS 12. |
| Native dependencies | Pass | `otool -L` lists only `/usr/lib` and `/System/Library` dependencies; no dynamic TDLib dependency. `nm -gU` confirms the static `_td_json_client_create` symbol. |
| Developer home paths | Pass | Binary `strings` scan found no `/Users/<name>/` or `/home/<name>/` paths. |
| Gatekeeper assessment | Expected rejection | Unsandboxed `spctl --assess --type execute --verbose=4` returned `rejected`, exit 3. The sandboxed attempt returned an internal subsystem error and was not used as policy evidence. The per-app **Open Anyway** installation flow is not yet exercised. |
| Production frontend | Pass | Existing local `dist` passed fixture/reset-IPC exclusion. Packaged fixture exclusion also passed the native CI packaging job. |
| Release/public checks | Pass | Nine release-metadata tests and the public-repository consistency check passed locally. |
| Keychain startup interaction | Owner reported completion | The owner completed two prompts and reported that the window opened. Initial accessibility inspection timed out while a call-stack sample confirmed `create_service` → `effective_live` → vault read → `SecItemCopyMatching`. No permission grant or password was automated. Prompt count on subsequent launch, ACL behavior and lock/denial cases remain untested. |
| Usable native workspace, initial candidate | Fail — release blocker | After Keychain access, native accessibility and screenshot inspection showed **Workspace not ready** / **This action belongs to a different account or source.** Clicking **Retry** reproduced the same error. Process inspection found only the tested candidate, with no competing Retract process. Subsequent synthetic investigation identified incompatible resealing during repository startup; fixed-candidate native validation is recorded separately below. Existing profile/Keychain contents were not cleared or bypassed. |
| Error-screen keyboard focus | Limited pass | Tab reached **Retry**, native accessibility reported its button label, and the screenshot showed a visible focus outline. This is not full-workspace keyboard or VoiceOver acceptance. |

## Native CI evidence

[Secure build run 38553380236](https://github.com/pryzm-labs/retract/actions/runs/38553380236), on the exact package source `75453d6e9e4bd20aa06bc2d5e96a034049fb7d78`, passed both Linux architectures and the native Apple-silicon macOS job. The native job's public log was inspected for the following results:

- Discord backend: 74 tests passed, zero failures.
- Archive persistence: 192 tests passed, zero failures, including encrypted database/WAL/journal plaintext-canary rejection, encrypted migration candidates, actual FTS-term cleanup, maintenance retry, and cancellation/shutdown draining.
- Injected vault/credential-lease tests: 28 passed, zero failures, including legacy preservation, cached startup denial, Remember/Forget key preservation, and competing-process ownership.
- Parser suites, locked installation, and unsigned production packaging passed.

These tests use synthetic data and injected vault I/O. They do not establish real Keychain ACL behavior, Touch ID/password fallback, screen-reader usability, or forensic erasure on a user's storage device.

## Reported provider testing

On 2026-10-14 the project owner reported that earlier Telegram and Discord tests had looked fine. This is recorded as historical user-reported smoke acceptance; it does not identify an exact revision or establish every role, cancellation, recovery, rate-limit, or second-participant case. No redundant live deletion was requested or performed here.

## Startup remediation

The owner authorized fixing startup without clearing saved data or credentials, followed by continued macOS validation. Two independent synthetic regressions reproduced `InvalidRequest("invalid Telegram execution recipe")` before the fix:

- Historical selected-message/delete-own-message envelopes are valid under their original `ResumeFrozenTargets` seal. `FoundationTelegramRepository::save` nevertheless rebound them using the current `RequiresNewReview` policy, changed their fingerprint, and rejected startup publication.
- The frozen authenticated version-3 encrypted fixture from the older source passed store validation but failed that same repository publication path. Previous migration tests exercised decoding, not the complete load/save path used to construct the engine.

The compatibility fix compares the entire validated historical projection against its previous sealed envelope, then preserves that envelope verbatim. Only newly prepared plans are bound using the current policy. No schema, vault, credential, or profile-reset path was changed; recovered destructive jobs remain unable to replay automatically. Regression coverage checks unchanged envelopes, account/source records, counters/cursors, terminal-history preservation, safe stopping of queued work, new-policy publication, and rejection of changed targets even when their ID/fingerprint are retained.

The full Linux arm64 Docker gate completed successfully (BuildKit `392rabgfyq89jgwzfdcfpkoiq`): 178 frontend tests, 502 backend tests with the existing one opt-in resource-test ignore, domain/parser suites, production/metadata/boundary checks, strict Clippy/formatting, and three benchmark-feature tests. Independent read-only code review found no critical or important issues; its additional changed-target coverage was included in this passing run.

### Fixed native candidate

- Package source: `18257c88ada9e88e9f1fd0569edb8f045a468af9`.
- ZIP SHA-256: `e6d9c0485d982754244b2d90b2f3b8f67ef3e43bad0b5566f79824f1facabd83`.
- Executable CDHash: `3fb634d749d9ff7fc0247b7242cbccdb357ddcd4`.
- Pinned-toolchain `npm run package:unsigned` passed on the local Apple-silicon host. Production fixture exclusion, extracted-bundle allowlists, arm64/static TDLib, system-only dependencies, developer-path exclusion, ad-hoc hardened signature and ZIP verification passed. The disposable native build directory was confirmed removed afterward.
- Independent checksum, strict signature, bundle-contents and deployment-target checks passed on the extracted fixed candidate. Gatekeeper assessment still returned the expected unsigned rejection; no bypass was applied.
- Before launch, a byte-identical copy of only the encrypted job-history file was retained in a private temporary directory with mode `0600`. No vault secrets or decrypted data were copied. With the previous app closed, the candidate was launched against the existing profile; the history file remained byte-identical while startup awaited the owner's Keychain response.
- The owner completed two Keychain prompts again and reported that the fixed candidate reached the chat list. Native accessibility inspection independently confirmed a usable Telegram workspace with existing scoped job activity and preserved legacy-history entries. The original startup failure is resolved on this host. The initial candidate's CI evidence does not establish a native test-suite run on the fix.

### Non-destructive native UI results

- Telegram connection settings open with configured credentials retained; the stored hash does not need re-entry. No field was changed and **Save settings** was not pressed. Tab navigation reached the hash, test-server checkbox, Cancel and Save controls; Escape dismissed the dialog. Full native pointer-trigger focus restoration was not established by the accessibility tool.
- Command-K focuses the message search field. Keyboard-only navigation reaches filters and job-details controls. Return opens a preserved job, focus moves to **Close job details**, and Escape returns focus to the original job control. The six outcome-counter labels and retained job/legacy-history views are accessible through the native accessibility tree. This does not replace a spoken VoiceOver test.
- The source chooser visibly showed **Loading imported accounts…**, then the existing Discord import appeared without reimporting the ZIP. This establishes real loading-state and retained-source discovery evidence.
- **Additional blocker:** selecting that retained Discord source failed. The UI subsequently showed only the source chooser with **Couldn’t load imported accounts** and a retry control, with no usable main workspace. Process inspection confirmed the same candidate remained running. `select_archive_v2` shuts down the current provider before `build_connection` succeeds; a failed construction can leave the previous provider stopped. The underlying Discord construction failure has not yet been established. No archive was reimported, removed or reset; no live deletion, token entry, Remember/Forget or credential edit was performed. Further source-transition remediation is required before this matrix can pass.

## Native coverage not independently established

The owner has accepted the previous manual testing as complete for this preview. The cases below remain useful additional coverage, not a request to repeat their full manual matrix. The separately authorized unsigned installation/update check is in progress above; published-artifact verification remains a release step. No unavailable case is represented as passed.

- Repeat cold-start/reopen observation across additional timing conditions if available. The captured stale-context race is fixed and one clean startup without Retry is verified on the startup-fixed candidate above; a general timing guarantee is not implied.
- Record same-build reopen prompt count and full upgrade/reinstall behavior. Two prompts and usable Telegram startup on the rebuilt identity are verified; no general Keychain ACL or prompt-count guarantee is implied.
- Touch ID/password fallback, cancel/expiry, and immutable-target reason. Never execute a real deletion merely to test authentication without separate approval.
- Real Keychain denied/locked behavior and final-identity migration/ACL/update behavior. Do not alter the owner's existing vault to manufacture these tests.
- Keyboard/focus, VoiceOver, 200% effective zoom, reduced motion, both themes, title-bar drag, and large confirmation plans in a short viewport.
- Quarantined downloaded-artifact **Open Anyway**, install/update/uninstall/reinstall. Do not disable Gatekeeper globally or remove retained user data without approval.
- macOS 12 baseline: unavailable on the current host; no compatibility pass is claimed.

See [TEST_PLAN.md](TEST_PLAN.md) and [release readiness](RELEASE_READINESS.md) for the full matrix. Keep issue #44 open until remaining outcomes are recorded or explicitly dispositioned.

## Discord source-opening remediation

The owner authorized fixing retained Discord source opening and recovery without clearing saved data or credentials. Synthetic regressions reproduced two independent defects: the persisted-state validator rejected supported historical `ResumeFrozenTargets` plan seals, and a failed candidate construction stopped the previous service, making subsequent source discovery/retry unusable.

Fix `4e58fe8f0a2623cbc7c434dc8291d1f400d536e2` separates historical storage validation from execution validation. Original supported v1 envelopes remain sealed and readable; authorization and mutation still require a newly reviewed `RequiresNewReview` plan. Recovery stops unfinished jobs rather than replaying them, while preserving terminal history, account/source rows and counters. Source selection now constructs and bootstraps its candidate before retiring the previous workspace; candidate bootstrap failure shuts down only the candidate. No schema, credential, Keychain, archive reset or removal path was changed.

Three focused regressions passed after their failures were reproduced. They cover encrypted Discord history reopen, rejection of tampered/unsupported or executable historical plans, no provider probes/deletes during recovery, failed registered source opening from setup and Telegram, saved Telegram plan/state preservation, corrected retry, same-scope Discord reselection and stale-session rejection. Registered IPC tests remain in the existing reviewed compatibility-test boundary; no lint exception was added. Independent read-only code review found no critical or important issue.

The full Linux arm64 Docker gate completed successfully (BuildKit `zcecywjp3h50dfma5up8ejycz`): 178 frontend tests, 505 backend tests with the existing one opt-in resource-test ignore, domain/parser suites, nine release tests, 14 fixture-audit tests, 31 boundary tests, production/public-repository checks, all formatting/strict Clippy and three benchmark-feature tests. Disk exhaustion during earlier attempts was resolved by removing only reproducible Retract build-cache records; no unrelated images/volumes or saved application data were removed.

Before native launch, a byte-identical mode-`0600` rollback copy of the existing encrypted Discord job-history file was retained in the private validation directory. No keys or decrypted data were copied.

### Discord-fixed native candidate

- Package source: `4e58fe8f0a2623cbc7c434dc8291d1f400d536e2`.
- ZIP SHA-256: `ebc6aa9b2e4be5d02312808983c2dc5d36ac4f8d8dbaf9e56dfc6e3d7b81a50f`.
- Executable CDHash: `eb5a621d48cd326fdc2ce00a729efa255281e6dc`.
- Pinned-toolchain unsigned packaging passed on the local Apple-silicon host. Production exclusion, bundle allowlists, ad-hoc hardened signatures, static TDLib, system-only runtime dependencies and developer-path exclusion passed; the disposable native build directory was confirmed removed.
- Independent ZIP checksum, extracted-bundle allowlist and strict signature verification passed. `file`, `LC_BUILD_VERSION` and Info.plist confirm arm64 and a macOS 12.0 deployment target, not execution on macOS 12. Read-only Gatekeeper assessment returned the expected unsigned rejection (exit 3); no protection was bypassed.
- The exact candidate was launched against the existing profile, and process inspection confirmed one instance of that candidate. Native accessibility inspection timed out while startup awaited Keychain access; the owner was asked to complete the prompts, never to disclose a password. Call-stack sampling confirmed `create_service` → `effective_live` → `SecItemCopyMatching` while awaiting owner interaction.

### Retained-source native revalidation

- The owner reported two Keychain prompts and **Workspace not ready** on initial startup; **Retry** then opened the Telegram chat list. Native accessibility independently confirmed the usable Telegram workspace and existing scoped/legacy cleanup history. The error heading is known, but its underlying diagnostic has not been captured. This is not a clean-startup pass.
- The source chooser showed its imported-account loading state, then the existing Discord import appeared without reimporting. Selecting it successfully opened a populated Discord workspace with grouped navigation, archived results and retained completed job activity. The earlier account/source and unsupported-version failures were not present.
- Opening Discord connection settings showed **Verified for this archive** and **stored in Keychain**, confirming remembered sign-in restored without token entry. No token was viewed, copied, replaced, forgotten or resaved.
- Server groups expanded without blanking the workspace. An actual server-channel row opened its populated, channel-filtered results. The unclassified group expanded and collapsed. A harmless local no-match keyword query showed **No messages found**; clearing it restored the selected channel's results. No message selection, review authorization or deletion occurred.
- With no active cleanup, the candidate was quit normally and process inspection confirmed exit. A same-build reopen was attempted to capture cold-start behavior. The owner pressed **Retry** before the diagnostic could be inspected; the native Telegram workspace was usable again afterward. The prompt count for this second probe was not supplied. At this stage, startup root cause and clean same-build reopen were unresolved; the subsequent captured diagnostic, narrow fix and clean launch are recorded above. No data/credential reset was made.
