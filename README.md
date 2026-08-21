# Retract

Retract is a local-first macOS desktop utility for finding and removing Telegram history with an explicit **delete for everyone** guarantee. It searches keywords across normal and secret chats, handles messages and media uniformly, makes cleanup authority visible, and supports selected-message deletion, whole-history clearing, explicit self-only chat-list removal, sender-wide moderation, and permanent group deletion when Telegram reports that the signed-in account has the exact capability. Dedicated **No reply sent** and **Empty** shortlists help surface likely spam or abandoned conversations for review.

The optional local **Privacy scan** walks message history in the selected scope without requiring a keyword. It flags likely email addresses, phone numbers, postal addresses, coordinates/location messages, contact cards, personal identifiers such as date-of-birth or record-ID language, identity-document references, checksum-valid payment cards and IBAN-like accounts, common cryptocurrency wallet formats—including Ethereum hex addresses, checksum-valid Bitcoin legacy and SegWit addresses, and context-associated 32-byte Solana public keys—IP addresses, and credential or recovery-phrase language. Findings are heuristics for human review: they can produce false positives and cannot guarantee that every sensitive item is found.

Retract never substitutes “delete for me” when an everyone-scoped deletion is unavailable or rejected.

## Current status

The application, safe fixture mode, TDLib adapter, encrypted job state, resumable execution, and confirmation flows are implemented. TDLib 1.8.64 is included for Apple silicon, loaded by an automated integration test, and placed in the macOS app bundle by the normal build. Live destructive behavior has not been exercised with Telegram credentials in this development environment, so it must pass [the test-DC release gate](docs/TEST_PLAN.md) before anyone uses a valuable production account.

A screenshot, photo, document, caption, or other attachment sent as a Telegram message is deleted with that message when Telegram accepts an everyone-scoped deletion. Separately saved screenshots/files, exports, quoted copies, forwards, notification history, third-party backups, and other copies outside Telegram may remain. Retract reduces the history Telegram still lets the current account revoke; it does not promise total erasure from the internet.

Privacy scanning examines message text, captions, filenames, contact-card fields, and Telegram location data. It does not currently download or OCR pixels inside images and videos, inspect document bodies, perform face/name recognition, or inspect copies outside Telegram.

## Safety model

- Plans freeze immutable chat/message IDs and a cryptographic fingerprint before confirmation.
- The Rust backend rechecks server-reported capability immediately before every selected-message deletion attempt.
- High and critical live actions require the exact chat title plus a fresh Touch ID or Mac login-password check. Its plan-bound grant expires after 60 seconds and is single-use.
- Everyone-scoped deletion requests use `revoke: true`; they never fall back to self-only deletion.
- **Remove chat for me** is a separate, explicitly confirmed operation. It uses `deleteChatHistory(remove_from_chat_list: true, revoke: false)` only when Telegram reports `can_be_deleted_only_for_self`; it removes history and the list entry only for the signed-in account.
- Whole-history cleanup also asks Telegram to remove the conversation from the current account’s chat list. A later message can recreate a DM, and clearing a group’s history does not itself dissolve the group.
- Group/channel leave cleanup favors the broadest authority Telegram currently reports. A whole-history-capable admin gets **Clear all history & leave**; otherwise an admin who can delete others’ messages gets **Delete all possible history & leave**, which freezes every enumerated message ID and deletes each still-eligible item in batches. Regular members get **Revoke my messages & leave**, scoped only to their outgoing IDs. All three paths clean first and leave second. Protected or rejected messages are reported without a self-only fallback, and **Delete group permanently** remains a separate owner-only critical action.
- Batches are capped at 100 messages. Telegram flood waits persist and resume, and jobs can be cancelled between calls.
- Job state is AES-256-GCM authenticated and encrypted. The API hash, job-store key, and TDLib database key share one versioned macOS Keychain vault that Retract reads once and retains only in zeroizing process memory. The first launch after upgrading from the legacy three-item layout may request access to each old entry once while migrating them; later launches access only the consolidated vault.
- Job records contain IDs, counters, states, and normalized errors—not message bodies.
- Privacy findings are computed locally and attached only to the in-memory search result; matched values are not added to job logs.
- The webview has a strict content-security policy and only Tauri core permissions.

See [THREAT_MODEL.md](docs/THREAT_MODEL.md) for the trust boundaries and residual risks.

## Run safe demo mode

Requirements: Apple-silicon macOS 12+, Node.js 20+ with npm, a current stable Rust toolchain, and the Xcode Command Line Tools. No manual TDLib build or library-path configuration is required.

```sh
npm install
npm run tauri dev
```

On first launch, choose **Safe demo** in Retract's setup window. The app is then confined to disposable local fixtures. The fixtures include an untouched spam-like DM and a confirmed-empty DM so the new cleanup shortlists can be tested safely. The browser-only UI can be run with `npm run dev`, but native storage, TDLib, Keychain, and macOS owner authentication are available only in the Tauri app.

For the first end-to-end verification with another person, follow [LOCAL_LIVE_TEST.md](docs/LOCAL_LIVE_TEST.md). A new two-person private group is safer than using an existing DM because it contains no unrelated history.

## Connect Telegram from the UI

Retract includes TDLib **1.8.64** built from the exact official commit recorded in `vendor/tdlib-dist/build-stamp.txt`. `npm run tauri dev`, `npm run tauri build`, and `npm run check` verify the bundled library's architecture and SHA-256 automatically. If the artifact is absent or does not match the current Mac architecture, `scripts/ensure-tdlib.sh` fetches that exact source revision and rebuilds it; only that uncommon rebuild path needs CMake, gperf, and Homebrew OpenSSL 3.

Obtain your own application ID/hash from [Telegram’s application page](https://my.telegram.org), then launch Retract normally:

```sh
npm run tauri dev
```

In the first-run window, choose **Connect Telegram**. The screen should show **TDLib 1.8.64 included — READY**. Enter:

1. The numeric Telegram API ID.
2. The 32-character Telegram API hash.
3. Whether to use Telegram's separate test server.

Choose **Save settings**, then authorize with the recommended QR flow or your phone/code/2FA flow. The new profile is applied without terminating the app. The API hash is stored in macOS Keychain. The ordinary settings file contains only the mode, API ID, test-server choice, and any advanced custom-library override—not the API hash. The Settings button beside **CHATS** and the **Configure Telegram** demo banner reopen this screen.

Environment variables remain optional developer/CI overrides and take precedence when present. `.env.example` documents them; Vite does not inject these values into the frontend. Do not prefix them with `VITE_`, commit them, or pass them to browser code. Retract automatically isolates demo, test-DC, and production databases and durable job stores.

Live authorization supports QR login and phone/email/code/2FA states. TDLib session data is written under the operating system’s app-local-data directory with database encryption enabled.

## Verification

The default isolated verification path is Docker BuildKit. It uses
digest-pinned official Node 24.19.0 and Rust 1.97.1 images, installs from the
committed npm and Cargo lockfiles, disables npm dependency lifecycle scripts,
and runs every project build, test, format, and lint command as a non-root user
with networking disabled:

```sh
npm run container:check
```

The check target uses Docker's cache-only exporter, so it never leaves a
multi-gigabyte runnable test image behind. npm downloads and Cargo dependencies
use named BuildKit caches, while architecture-specific Rust target output stays
in one reusable cache instead of being copied into a new immutable layer after
every source edit. CI checks also omit incremental compilation and debug symbols,
which are unnecessary for lint/test validation and substantially reduce disk
usage. Inspect Docker's current footprint at any time with `docker system df`.
Retract does not automatically prune Docker because the cache may be shared with
other projects; Docker's normal BuildKit garbage collection remains in control.
To inspect only Retract's named caches, run `npm run container:cache:status`. If
you no longer need the faster rebuild cache, `npm run container:cache:prune`
offers an interactive, project-scoped cleanup and leaves other projects' cache
mounts alone.

To export the compiled frontend from the same verified container stage:

```sh
npm run container:build
```

The files are written to `artifacts/frontend/`. No Telegram credentials, `.env`
files, local databases, TDLib session data, Docker socket, or host directories
are included in the build context. Do not pass Telegram credentials as Docker
build arguments; neither verification nor packaging needs them.

Docker substantially isolates dependency and compiler execution from the host,
but it cannot prevent every toolchain or supply-chain issue: the Docker daemon,
base-image contents, Debian package repository, and build output remain trusted.
A compromised dependency could still alter the artifact even though it cannot
reach the host or network during compilation. Rootless Docker is recommended on
Linux. Review lockfile changes and container base-image digest updates like
source changes.

Native desktop packaging remains OS-specific. Docker verifies the portable
frontend and Rust/Tauri code on both amd64 and arm64 Linux; GitHub Actions then
builds the macOS `.app` on an ephemeral Apple-silicon runner because Xcode,
Apple frameworks, signing, and notarization cannot run in a Linux container.
The workflow has read-only repository permission, persists no checkout token,
uses full-SHA-pinned actions, receives no Telegram secrets, and only starts the
native package after both container architectures pass. Windows and Linux
installers are not release targets yet because Retract's high-impact
device-owner authentication is deliberately macOS-only.

For faster host-native development checks, the existing commands remain:

```sh
npm run check
cargo clippy --manifest-path crates/cleaner-domain/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm audit --audit-level=high
```

The complete manual destructive matrix is in [TEST_PLAN.md](docs/TEST_PLAN.md). Do not treat a successful compile or fixture test as evidence that Telegram accepted an operation.

## Architecture

- React + TypeScript renders the keyboard-accessible three-pane UI.
- Tauri 2 exposes a small local IPC command surface.
- Rust owns plan validation, confirmations, encrypted durability, retries, cancellation, and all deletion calls.
- `cleaner-domain` is a Telegram-independent crate containing capability and plan invariants.
- TDLib is accessed through its JSON C interface; message IDs make deletion content-agnostic across text, photos, video, files, voice, audio, animation, stickers, polls, locations, contacts, and service content.

The detailed product and engineering rationale remains in [PLAN.md](PLAN.md).
