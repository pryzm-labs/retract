# macOS 0.1.1 validation record

Recorded on 2026-10-14. This is evidence for the unsigned preview, not a claim that every manual acceptance case passed. No live messages were deleted during this validation.

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

## Still requiring native observation

- Fix and revalidate the newly observed retained-Discord-source transition failure, keeping both sources recoverable when a candidate connection cannot open.
- Record same-build reopen prompt count and full upgrade/reinstall behavior. Two prompts and usable Telegram startup on the rebuilt identity are verified; no general Keychain ACL or prompt-count guarantee is implied.
- Touch ID/password fallback, cancel/expiry, and immutable-target reason. Never execute a real deletion merely to test authentication without separate approval.
- Real Keychain denied/locked behavior and final-identity migration/ACL/update behavior. Do not alter the owner's existing vault to manufacture these tests.
- Keyboard/focus, VoiceOver, 200% effective zoom, reduced motion, both themes, title-bar drag, and large confirmation plans in a short viewport.
- Quarantined downloaded-artifact **Open Anyway**, install/update/uninstall/reinstall. Do not disable Gatekeeper globally or remove retained user data without approval.
- macOS 12 baseline: unavailable on the current host; no compatibility pass is claimed.

See [TEST_PLAN.md](TEST_PLAN.md) and [release readiness](RELEASE_READINESS.md) for the full matrix. Keep issue #44 open until remaining outcomes are recorded or explicitly dispositioned.
