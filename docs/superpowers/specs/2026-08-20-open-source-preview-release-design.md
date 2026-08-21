# Retract open-source preview release design

**Date:** 2026-08-20  
**Owner:** Pryzm Labs  
**Target:** Retract v0.1.0 source release with an unsigned Apple-silicon macOS preview

## Objective

Prepare Retract for a public GitHub repository that is straightforward to understand, build, inspect, and try. The source tree is the canonical distribution. Tagged versions may also publish a convenience macOS application archive, but that archive is deliberately not signed with an Apple Developer ID and is not notarized.

The public material must describe Telegram's actual deletion semantics precisely. Retract helps users delete the data Telegram still permits their account to revoke; it cannot guarantee removal of screenshots, forwards, exports, notification history, backups, moderation records, or copies outside Telegram.

## Decisions

- License Retract under the MIT License with Pryzm Labs as the initial copyright holder.
- Support Apple-silicon Macs running macOS 12 or later for v0.1.0.
- Do not require an Apple Developer account, Developer ID certificate, or notarization.
- Publish source and an explicitly labeled unsigned preview application archive.
- Keep the product's existing local-first architecture and UI-configured Telegram credentials.
- Do not expose Safe demo in a production build. Demo fixtures exist only for tests and generating synthetic screenshots.
- Keep permanent group dissolution as a separate destructive action; other cleanup actions continue to favor the broadest deletion scope Telegram currently reports.
- Treat v0.1.0 as a pre-release until the destructive integration matrix and an independent destructive-action review are complete.

## Release boundary

The public repository contains:

- React/TypeScript frontend and Tauri/Rust backend source.
- Locked npm and Cargo dependency graphs.
- The pinned Apple-silicon TDLib dynamic library, its upstream license, exact source commit, architecture, version, and checksum record.
- Docker-based portable verification and native macOS packaging workflows.
- Product, security, testing, contribution, and release documentation.
- Synthetic fixtures used by automated tests and screenshot generation.

The public repository and release artifacts must not contain:

- Telegram API credentials or authorization state.
- Keychain data, database encryption keys, job-store keys, or persisted jobs.
- TDLib session databases, message databases, exports, logs, or screenshots containing real conversations.
- Local build output, Docker caches, dependency directories, or generated TDLib source/build trees.
- Apple signing identities, signing secrets, or provisioning profiles.

## Production onboarding and demo isolation

A production build starts at Telegram connection setup when no usable live configuration exists. The user is guided to obtain an API ID and API hash from Telegram, enter them in Retract, and authorize their account. Safe demo is not shown as an onboarding choice, connection mode, banner, badge, settings option, or reset action.

Demo fixtures remain available to automated frontend/backend tests and to a dedicated development-only screenshot path. The production build must not be able to select or transition into the fixture gateway. If a development profile previously persisted Safe demo mode, a production build treats it as unconfigured and returns to Telegram setup.

Automated tests must establish both sides of this boundary:

- Development/test code can render deterministic fixtures for screenshots and assertions.
- A production-mode build exposes only the Telegram connection path and rejects or migrates persisted demo configuration.

The README screenshot uses synthetic names and messages and is captioned as synthetic fixture data. It does not advertise a demo feature to end users.

## Public README and onboarding

The README should be useful in this order:

1. Retract logo, concise purpose, platform/license/status badges, and an honest pre-release warning.
2. A polished screenshot made exclusively from synthetic fixture data.
3. A short feature summary covering global search, privacy scanning, media-aware deletion, authority visibility, self-only chat removal, group-leave cleanup, and owner-only group dissolution.
4. Deletion and privacy limits in plain language.
5. Two installation paths:
   - Download the unsigned Apple-silicon preview and approve only that application through macOS Privacy & Security.
   - Build the tagged source locally with pinned Node/Rust versions and Xcode Command Line Tools.
6. First launch: obtain Telegram application credentials, configure them in the UI, authorize Telegram, and begin with an isolated DM or newly created private group.
7. Docker verification, architecture, security model, contribution links, and third-party licensing.

The README must never advise users to disable Gatekeeper globally or remove quarantine attributes broadly. It must explain that macOS cannot verify an unsigned preview's developer and that building from reviewed source is the higher-trust option.

## Repository standards

Add and maintain:

- Root `LICENSE` containing the standard MIT license.
- `SECURITY.md` describing private vulnerability reporting and forbidding sensitive Telegram data in public reports.
- `CONTRIBUTING.md` with setup, checks, pull-request expectations, and destructive-test restrictions.
- `CODE_OF_CONDUCT.md` using a standard community code of conduct.
- `CHANGELOG.md` with a v0.1.0 pre-release entry.
- Third-party notices covering bundled TDLib and other vendored material.
- Maintainer-facing release instructions.
- GitHub issue forms for bugs and feature requests, a pull-request template, and a security-report redirect.
- Dependabot configuration for npm, Cargo, GitHub Actions, and Docker dependencies.
- Consistent MIT metadata in npm and both Cargo manifests.

Repository metadata should target the Pryzm Labs organization and repository name `retract`. Until the remote is created, links that require the exact organization slug must either remain relative or be derived from the configured remote rather than guessed.

The initial branch is renamed from `master` to `main`. Local/session/build exclusions are verified before the first complete source commit.

## Verification and CI

Pull requests and pushes to `main` run the existing digest-pinned Docker verification on Linux amd64 and arm64. Project-controlled compilation and tests remain non-root and network-disabled after the locked dependency acquisition stages. Native packaging runs on GitHub's Apple-silicon macOS runner only after portable checks pass.

CI verifies:

- Frontend tests and production build.
- Rust domain/backend tests, formatting, and clippy with warnings denied.
- TDLib version, architecture, checksum, and dynamic loading.
- The production demo isolation tests.
- Native application bundle integrity and ad-hoc hardened-runtime signature.
- The absence of known secret/session/build artifacts from the tracked tree and packaged archive.

No workflow receives Telegram credentials or Apple signing material. Checkout credentials are not persisted, workflow permissions remain read-only by default, and actions remain pinned to full commit SHAs.

## Unsigned preview release

A reproducible local command creates the same unsigned Apple-silicon application package used in CI. A version tag matching the manifests triggers the full verification chain and then produces:

- `Retract-vX.Y.Z-macos-arm64.app.zip`
- A SHA-256 checksum file.
- A machine-readable build manifest containing the source commit, application version, target architecture, minimum macOS version, TDLib version/commit/checksum, and unsigned/notarization status.
- GitHub build-provenance attestation when supported for the public repository.

The workflow verifies the archive before upload and creates a GitHub pre-release. The release notes prominently state:

- Apple silicon and macOS 12+ only.
- Ad-hoc signed, not Developer ID signed, and not notarized.
- Expected Gatekeeper approval steps.
- Source-build alternative.
- Pre-release/destructive-operation warning.

Release publication is the only workflow job granted `contents: write`; provenance receives only the narrowly required identity/attestation permissions. Release publication must fail closed on test failure, tag/version mismatch, wrong architecture, missing TDLib provenance, unexpected signing identity, or an archive-content policy violation.

## Security and public-release review

Before the first complete source commit intended for publication:

- Run the full existing test/check suite from a fresh Docker build.
- Run a repository-wide secret and private-artifact audit without printing possible secret values.
- Perform a standard repository security review and address validated high-impact findings.
- Review dependency advisories and document anything accepted for the preview.
- Inspect the final application archive and build manifest.
- Render and visually review the README, logo, and synthetic screenshot.

The existing threat model remains public. Its unresolved release-hardening list is not hidden; signed/notarized distribution, independently reproduced TDLib provenance, complete test-DC execution, SBOM/native vulnerability automation, and an independent destructive-action assessment remain clearly tracked.

## Failure handling

- Failure to load a valid live configuration returns the user to Telegram setup; it never falls back to fixtures.
- Failure to validate a release artifact prevents publication.
- A GitHub pre-release can be withdrawn, but Telegram deletions already accepted by the service cannot be rolled back.
- Documentation avoids promising restoration, total erasure, guaranteed PII detection, or universal deletion authority.
- Unsupported Intel Macs receive a clear platform error or documentation stop rather than an incompatible download.

## Acceptance criteria

- A new Apple-silicon macOS user can clone the repository, follow the README, launch Retract, configure Telegram entirely in the UI, and use an isolated test conversation without undocumented environment variables or a manual TDLib build.
- A user can download the unsigned preview and follow Apple's per-application approval flow without disabling system-wide protections.
- The production application contains no user-facing demo route and cannot load fixture mode from persisted settings.
- All documented commands are verified from clean state.
- GitHub-facing policies, templates, metadata, license, screenshot, and third-party notices are present and internally consistent.
- CI and release workflows use least privilege, pinned actions, locked dependencies, and no application secrets.
- The tagged release artifact is traceable to source and TDLib provenance and is unmistakably labeled unsigned and pre-release.
