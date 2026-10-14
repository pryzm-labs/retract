# Changelog

All notable changes to Retract are documented here. The project follows [Semantic Versioning](https://semver.org/), with preview releases allowed to change before 1.0.

## [Unreleased]

## [0.1.1] - Planned

Prepared for an unsigned prerelease. The tag and GitHub release have not been published; manual provider and macOS acceptance remains pending. See [release readiness](docs/RELEASE_READINESS.md).

### Added

- Scoped opaque identities and version-2 IPC on the Telegram UI, backed by a provider registry and the existing Telegram execution engine.
- Authenticated account verification, stable account/source mappings, session-bound plan authorization, and fail-closed cross-account recovery.
- A single-writer version-3 encrypted job store with exact legacy ciphertext preservation in `jobs.pre-provider.enc`; unfinished legacy work requires new review and the backup is never restored automatically.
- Backend-described ordered cleanup effects and accessible complete job outcomes, including skipped, failed, uncertain, retry and blocked states.
- Synthetic lifecycle, migration, interprocess-lock and stale-response regression coverage.
- A lazy, bounded SQLCipher archive backend with scoped Discord Data Package import, retry, query, source removal, cancellation and application-lifetime shutdown ownership. Telegram is not automatically indexed and providers other than Telegram and Discord are not enabled.
- A shared fail-fast macOS credential lease, lazy archive-key vault upgrade and injected process/lifecycle tests. All older Retract copies must quit before first archive use; vault-format downgrades are unsupported.
- An opt-in 100,000-item synthetic same-worker storage benchmark and complete synthetic archive/injected-vault native pre-package gates.
- Discord source selection, archive search and privacy scanning, grouped direct-message/server navigation, import recovery and remembered-source restoration.
- Account-bound Discord authentication through browser-neutral manual token entry, with memory-only sessions by default and optional macOS Keychain persistence.
- Immutable exact-message Discord cleanup plans with typed confirmation, device-owner authorization, bounded pacing/retries and content-free durable outcomes.

### Fixed

- Updated the development-only `source-map-js` transitive dependency to 1.2.2 for GHSA-68fv-2mgg-jv7q (indexed source-map denial of service).
- TDLib native logging now defaults to verbosity `0`, preventing its internal error logger from printing private message-derived URLs unless a developer explicitly opts into native diagnostics with `RETRACT_TDLIB_LOG_VERBOSITY`.
- Telegram privacy scans now follow a valid continuation cursor after TDLib omits every message from a converted page, ignore approximate `total_count` values, and stop on repeated cursors or an excessive page count.
- Discord deletion rechecks live ownership, timestamps, text and attachment identity against the frozen archive evidence before deleting each exact message.
- Telegram and Discord cancellation persists before stopping execution and waits for active mutation gates to drain; stale session restoration cannot undo a concurrent Forget.
- Native authorization identifies the immutable target and a 64-bit plan token, with bounded labels that remove invisible formatting controls.
- Ambiguous broad Telegram operations require a new review after restart instead of replaying against later history.
- Network responses and archive inputs enforce bounded reads and reject unsafe file types.

### Changed

- Aligned Rust Tauri, the JavaScript API and CLI on 2.12.1, including dialog-plugin 2.8.0; refreshed the frontend tooling and icons, Vitest 5.0.1, domain UUID/thiserror and the development WebSocket dependency.
- TDLib 1.8.64 is statically linked from a pinned, integrity-checked archive; runtime settings cannot select a native library path.
- Browser-debug Discord token capture is disabled in release builds; synthetic browser compatibility harnesses remain available in development.
- Local and GitHub Docker builds share a tracked-files-only context wrapper, target-specific dependency downloads and reusable named caches. Temporary build contexts are cleaned after success or failure.
- Unsigned macOS packaging builds in a disposable directory and rejects developer-machine paths in release metadata and bundled native dependencies.

## [0.1.0] - 2026-08-20

### Added

- Local-first Telegram search across chats and common media metadata.
- Sensitive-information scanning, including Ethereum, Bitcoin, and Solana wallet formats.
- Capability-checked selected-message, history, leave, and administrator cleanup workflows.
- Profile-bound encrypted cleanup jobs, frozen-ID resume, and fail-closed handling for ambiguous broad-operation and permanent group-deletion restarts.
- Single-use macOS authorization with a backend-derived target description for every destructive plan.
- Backend-resolved sender identities and cancellation checks immediately before destructive TDLib calls.
- UI-based Telegram setup with bundled Apple-silicon TDLib 1.8.64.
- Synthetic-only fixture mode for automated tests and project screenshots.
- Reproducible, ad-hoc-signed unsigned preview packaging for macOS 12+ on Apple silicon.

[Unreleased]: https://github.com/Pryzm-Labs/retract/compare/v0.1.0...HEAD
[0.1.1]: https://github.com/Pryzm-Labs/retract/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Pryzm-Labs/retract/tree/v0.1.0
