# Retract 0.1.1 release readiness

Prepared on 2026-10-14. This document is the current release checklist; the larger product roadmap remains in [PLAN.md](../PLAN.md). Version `0.1.1` is reserved in the application manifests. No `v0.1.1` tag or GitHub release has been created. The existing `v0.1.0` tag points to older source and must not be moved.

## Completed evidence

- Telegram and Discord implementation, archive recovery/navigation, remembered sessions and destructive-workflow hardening are merged through [PR #36](https://github.com/pryzm-labs/retract/pull/36), `d926d8e`.
- The baseline deep scan reported 19 findings (1 high, 8 medium, 10 low). Remediation and the final review cancellation/session fixes landed in PR #36 with regression coverage. This is fix/review evidence, not a claim of a new clean repository scan.
- [Secure build run 36274011228](https://github.com/pryzm-labs/retract/actions/runs/36274011228) passed on reviewed branch revision `8f2ed78e420c0b3f8680f167e7c276973513702e`, subsequently squash-merged as `d926d8e`. Linux amd64, Linux arm64 and native Apple-silicon tests/unsigned packaging all passed.
- Local unsigned packaging also passed production fixture exclusion, file allowlists, ad-hoc signature, hardened runtime and developer-path checks.
- Documentation includes the logo, synthetic screenshot, source setup, unsigned install flow and provider limitations. Release builds use manual Discord token entry.
- Release preparation and dependency integration merged in [PR #50](https://github.com/pryzm-labs/retract/pull/50), `aa6ccfb`. [Secure build run 38553380236](https://github.com/pryzm-labs/retract/actions/runs/38553380236) passed Linux amd64, Linux arm64, and native macOS tests/unsigned packaging on its reviewed PR head `75453d6`; the merged and reviewed commits have identical source trees. Dependency issue #41 and the seven original dependency PRs are closed.
- The [macOS 0.1.1 validation record](MACOS_VALIDATION_0.1.1.md) records fresh local checksum, signature, bundle, dependency, deployment-target and Gatekeeper assessment results, native CI privacy/vault evidence, and the remaining interactive checks.

## Remaining release gates

Tracked in the [0.1.1 unsigned preview milestone](https://github.com/pryzm-labs/retract/milestone/1), without a fixed release date.

| Gate | Current state | Completion evidence |
| --- | --- | --- |
| Release-preparation PR | Complete | PR #50 merged; version/lockfile checks and all three CI jobs passed |
| [Dependency review #41](https://github.com/pryzm-labs/retract/issues/41) | Complete | Combined candidates merged in PR #50; original dependency PRs and issue #41 closed |
| [Telegram disposable test-DC acceptance #42](https://github.com/pryzm-labs/retract/issues/42) | Historical smoke pass reported by owner | Earlier tests reported successful on 2026-10-14; exact revision and complete role/recovery matrix not supplied |
| [Discord harmless exact-message acceptance #43](https://github.com/pryzm-labs/retract/issues/43) | Historical smoke pass reported by owner | Earlier tests reported successful on 2026-10-14; exact revision and complete cancellation/rate-limit matrix not supplied |
| [Native macOS credentials, privacy and accessibility #44](https://github.com/pryzm-labs/retract/issues/44) | Retained Discord opening and rebuilt clean startup verified; broader matrix open | Startup/history/source fixes passed independent review and the complete Linux arm64 Docker gate at `c61af59`, including 194 frontend/505 backend and three benchmark-feature tests. Native package `cb85f6d` passed artifact checks; the owner completed two prompts and native inspection confirmed the Telegram chat list without Retry and restored Discord import/sign-in. Broader credentials/accessibility/install checks remain in the validation record |
| [Final tag and unsigned prerelease #45](https://github.com/pryzm-labs/retract/issues/45) | Pending | Final source passes CI and manual gates, published ZIP/checksum/manifest/provenance, independent downloaded-artifact verification |

Keep manual evidence content-free: source commit, platform, role, fixed outcome classes and pass/fail are sufficient. Do not put credentials, tokens, exports, account identifiers, private content or screenshots in public issues. Existing exploratory use is useful feedback but does not close the complete acceptance matrix.

### Dependency candidates

PR #50 integrates #30 (domain UUID 1.26.1), #33 (Vitest 5.0.1), #34 (tokio-tungstenite 0.30.0), #37 (thiserror 2.0.21), #38 (Rust Tauri 2.12.1/dialog 2.8.0) and #40 (npm API/CLI 2.12.1, icons, React plugin, jsdom and Vite updates). Their lockfile overlaps retain the application version `0.1.1` and the reviewed newer dependencies. #32's Tauri 2.11.6 update is superseded by the combined 2.12.1 update.

The individual #38 and #40 macOS jobs failed because one side of Tauri moved to 2.12 while the other remained on 2.11. PR #50 aligned both sides, passed all three required CI jobs, and merged. The original PRs are closed; issue #41 records the final result.

The candidate also refreshes development-only `source-map-js` from 1.2.1 to 1.2.2, using the registry's published tarball/integrity, for [GHSA-68fv-2mgg-jv7q](https://github.com/advisories/GHSA-68fv-2mgg-jv7q). Both parent dependency ranges admit the patch; its acceptance includes a clean lockfile advisory check and locked installation in Docker.

The minimum supported platform is Apple-silicon macOS 12. Verify that baseline and a current macOS release where available, recording unavailable coverage honestly. Paid Apple signing and notarization are not prerequisites for this unsigned preview.

## Publication procedure

1. Review dependency updates and rerun affected checks. Keep the reviewed source and evidence tied to an immutable commit.
2. Complete the manual gates above and record their outcomes. If code changes during validation, rerun the affected cases and CI.
3. Merge release preparation, finalize the `0.1.1` changelog date, and update its comparison link to the eventual tag. Confirm package, npm lockfile, Tauri, application Cargo and domain Cargo versions agree; internal archive schema/locator versions are separate contracts.
4. Create and push a new `v0.1.1` tag on the approved commit. The [release workflow](../.github/workflows/release.yml) checks tag/version consistency, runs both Linux architectures and native macOS packaging, and publishes an unsigned prerelease with checksum, manifest and build provenance.
5. Download the published set, verify its checksum/provenance and source revision, and test installation through the documented per-app Gatekeeper flow. Record that independent check before announcing the preview.

## Follow-up improvements

These are useful product work, rather than prerequisites for this preview:

- [Archive-open retry #46](https://github.com/pryzm-labs/retract/issues/46): retry from the UI after correcting Keychain or profile contention, without requiring a complete restart.
- [OCR/document detection #47](https://github.com/pryzm-labs/retract/issues/47): opt-in local processing with explicit resource limits.
- [Discord export formats #48](https://github.com/pryzm-labs/retract/issues/48): enable additional formats only after structural evidence and synthetic fixtures establish their contracts.
- [Linux/Windows packaging #49](https://github.com/pryzm-labs/retract/issues/49): evaluate native credential/authorization replacements and platform artifacts. The current bundled TDLib artifact is Apple-silicon macOS only.
