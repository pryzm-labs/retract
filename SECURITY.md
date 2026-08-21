# Security policy

Retract performs irreversible operations on private Telegram data. Please treat security and deletion-scope bugs as sensitive.

## Reporting a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/Pryzm-Labs/retract/security/advisories/new). Do not open a public issue for a vulnerability before it is resolved.

Never include Telegram API credentials, authorization codes, session files, TDLib databases, chat exports, unredacted conversations, or screenshots of private conversations in any report. Create a minimal synthetic reproduction and redact device usernames and local paths.

Include the Retract version or commit, macOS version and architecture, expected deletion scope, observed result, and the smallest safe reproduction you can provide. We will acknowledge a report when maintainers are available; this preview project does not promise a fixed response SLA.

## Supported versions

The latest tagged preview is the only supported version. Retract v0.1.x targets Apple-silicon Macs running macOS 12 or newer. Preview archives are ad-hoc signed but not Apple-notarized; verify the published SHA-256 checksum or build from source.

## Security boundaries

Retract can request only the operations Telegram currently authorizes for the signed-in account. It cannot erase forwarded content, exports, notification history, screenshots, backups, or copies held outside Telegram. See the [threat model](docs/THREAT_MODEL.md) for the detailed boundaries and residual risks.
