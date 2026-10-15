# Releasing Retract

Retract preview releases are Apple-silicon macOS application archives with an ad-hoc signature. They are not notarized and do not require an Apple Developer account. GitHub releases must be marked as pre-releases until the project explicitly adopts a stable release policy.

## Prepare the release

1. Update the version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `crates/cleaner-domain/Cargo.toml`, and `crates/retract-domain/Cargo.toml`. All five values must match exactly; update the corresponding lockfiles with the pinned toolchains.
2. Finalize the changelog date and comparison links, review dependency/native-provenance changes, commit the release changes, and ensure the working tree is clean. Application versions are separate from archive schema and provider-locator versions; do not bump those formats just to match a release number.
3. Run the Docker gates and require both Linux architectures plus native macOS CI on the reviewed source, as described in [TEST_PLAN.md](TEST_PLAN.md). They include production-bundle, version/license, provider-boundary and release-metadata checks.
4. Record manual provider/native acceptance in the release PR: source revision, environment, outcomes, CI links and limitations. Distinguish owner acceptance of earlier testing from independently observed cases. Do not call an unavailable macOS version, denied/locked Keychain condition or accessibility case a pass. Review security findings and required fixes before approving the release.
5. On an Apple-silicon Mac, run `npm run package:unsigned` for local artifact/installation checks. The workflow separately rebuilds the published package from the approved tag.

The packaging command builds the normal production app, requires an arm64-only executable, verifies the hardened ad-hoc signature, checks the pinned TDLib checksum and runtime dependencies, expands the archive into a fresh directory, and verifies the expanded copy again.

## Inspect the output

Artifacts are written to `artifacts/release/`:

- `Retract-vX.Y.Z-macos-arm64.app.zip`
- `Retract-vX.Y.Z-macos-arm64.app.zip.sha256`
- `Retract-vX.Y.Z-macos-arm64.app.zip.manifest.json`

Review the manifest and verify the checksum before publishing. The manifest must identify the intended version and immutable source commit, `aarch64-apple-darwin`, macOS 12.0, ad-hoc signing, `notarized: false`, and the reviewed TDLib revision and digests.

## Installation and update validation

Use an actual browser-downloaded ZIP so macOS quarantine is exercised; a locally built unquarantined app is not equivalent. Verify the checksum, extract in Finder and copy the app to Applications. Follow the documented [per-app Open Anyway flow](../README.md#install-an-unsigned-preview) yourself; never disable Gatekeeper globally or strip quarantine to report a pass.

Before testing an update, quit all older Retract instances with no active cleanup. Retain the previous app bundle for diagnosis, then replace only the application—not its saved profile or Keychain items. Verify the installed candidate's signature and expected source/build identity. Let the owner complete any authentication prompts without disclosing a password. Confirm Telegram and Discord source discovery, saved history and optional remembered Discord sign-in survive without Retry, reimport, credential re-entry or reset; record any exception and the observed prompt count. A build-to-build replacement within one version does not establish an older-release migration.

Downgrading is not a supported recovery procedure: old readers cannot read active v3 job state or a newer vault. Preserve invalid state for diagnosis rather than clearing it or restoring an older backup. See [storage compatibility](STORAGE.md#job-state-and-migration).

## Tag and publish

Merge the reviewed release preparation and obtain release-owner approval before creating a new annotated tag. Never move an existing published tag. Only after the checks pass:

```sh
git tag -a vX.Y.Z -m "Retract vX.Y.Z"
git push origin main vX.Y.Z
```

The [tag workflow](../.github/workflows/release.yml) checks tag/version consistency, runs both container architectures, rebuilds and verifies the unsigned archive on macOS, attests all three release files and creates a GitHub pre-release. Do not upload locally modified artifacts to replace workflow output.

Release notes must state that the app is unsigned/unnotarized, Apple-silicon-only, and intended for users who are comfortable building from source or verifying an open-source preview archive.

## Verify the published download

Before announcing the preview, download the published ZIP, checksum and manifest independently. Verify the ZIP checksum, the manifest's source commit against the approved tag, and GitHub build provenance against this repository and its release workflow. Verify the extracted signature/bundle contents and exercise the quarantined per-app launch on the supported Mac. A pre-publication local package check cannot establish provenance or bytes of the later GitHub download.

Keep the content-free results and any unavailable coverage in the release PR/notes. Never attach private exports, credentials, account identifiers, session databases or real chat screenshots. Future work belongs in GitHub issues, not per-release readiness or validation diaries in the source tree.
