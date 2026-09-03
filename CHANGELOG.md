# Changelog

All notable changes to Retract are documented here. The project follows [Semantic Versioning](https://semver.org/), with preview releases allowed to change before 1.0.

## [Unreleased]

### Added

- Scoped opaque identities and version-2 IPC on the Telegram UI, backed by a provider registry and the existing Telegram execution engine.
- Authenticated account verification, stable account/source mappings, session-bound plan authorization, and fail-closed cross-account recovery.
- A single-writer version-3 encrypted job store with exact legacy ciphertext preservation in `jobs.pre-provider.enc`; unfinished legacy work requires new review and the backup is never restored automatically.
- Backend-described ordered cleanup effects and accessible complete job outcomes, including skipped, failed, uncertain, retry and blocked states.
- Synthetic lifecycle, migration, interprocess-lock and stale-response regression coverage. No additional production provider, content index or archive importer is included.

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
[0.1.0]: https://github.com/Pryzm-Labs/retract/releases/tag/v0.1.0
