# Releasing Retract

Retract preview releases are Apple-silicon macOS application archives with an ad-hoc signature. They are not notarized and do not require an Apple Developer account. GitHub releases must be marked as pre-releases until the project explicitly adopts a stable release policy.

## Prepare the release

1. Update the version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `crates/cleaner-domain/Cargo.toml`. All four values must match exactly.
2. Update `CHANGELOG.md`, commit the release changes, and ensure the working tree is clean.
3. Run `npm run container:check` and `npm run verify:production-bundle`.
4. On an Apple-silicon Mac, run `npm run package:unsigned`.

The packaging command builds the normal production app, requires an arm64-only executable, verifies the hardened ad-hoc signature, checks the pinned TDLib checksum and runtime dependencies, expands the archive into a fresh directory, and verifies the expanded copy again.

## Inspect the output

Artifacts are written to `artifacts/release/`:

- `Retract-vX.Y.Z-macos-arm64.app.zip`
- `Retract-vX.Y.Z-macos-arm64.app.zip.sha256`
- `Retract-vX.Y.Z-macos-arm64.app.zip.manifest.json`

Review the manifest and verify the checksum before publishing. The manifest must identify `aarch64-apple-darwin`, macOS 12.0, ad-hoc signing, no notarization, and the reviewed TDLib revision and digest.

## Tag and publish

Create an annotated tag only after the checks pass:

```sh
git tag -a vX.Y.Z -m "Retract vX.Y.Z"
git push origin main vX.Y.Z
```

The tag workflow rebuilds the unsigned archive from the tagged source and creates a GitHub pre-release. Do not upload locally modified artifacts to replace workflow output.

Release notes must state that the app is unsigned/unnotarized, Apple-silicon-only, and intended for users who are comfortable building from source or verifying an open-source preview archive.
