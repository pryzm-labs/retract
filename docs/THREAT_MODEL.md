# Threat model

## Objective

Retract helps a user revoke history that Telegram or Discord still permits the matching account to delete. The primary security property is scope integrity: the app must perform only the operation the user reviewed, against immutable provider/account/source IDs, and must never quietly widen targets or weaken “for everyone” into “for me.”

## Protected assets

- Telegram authorization/session state and API credentials, and Discord user-session tokens.
- The TDLib database encryption key and encrypted job-store key.
- Imported Discord archives, archive encryption keys, and private message metadata shown in memory.
- Frozen deletion plans and their execution status.
- The user’s ability to distinguish owner/admin/member authority and deletion reach.

## Trust boundaries

1. **React webview:** untrusted for destructive authorization. It displays plans and gathers intent but cannot mint a valid backend plan or system-authentication grant.
2. **Rust core:** trusted enforcement boundary. It validates IDs and capabilities, freezes plans, checks confirmation proofs, consumes system grants, persists jobs, and calls the gateway.
3. **TDLib dynamic library:** trusted native dependency pinned by version and exact release commit. The normal consumer build bundles it as an app resource; the build verifies its recorded SHA-256 and the adapter verifies the loaded version. A substituted library still executes inside the application process, so users must verify preview provenance or build from reviewed source.
4. **Telegram and Discord services:** authoritative for current access and deletion outcomes. Capabilities and membership may change between archived evidence, preview, and execution. Discord normal-user automation is unsupported and can result in account enforcement.
5. **Temporary browser process and loopback automation endpoint:** untrusted token-capture transport. Retract launches a fresh profile, binds automation to loopback, filters protocol events to HTTPS Discord API requests, extracts only the selected authorization value, and terminates the child on completion, cancellation, or timeout. Browser compromise can still disclose or substitute a token.
6. **macOS Keychain and LocalAuthentication:** trusted for optional secret storage and fresh device-owner verification.
7. **Local filesystem:** considered observable or modifiable by other software running as the user. Discord ZIPs are selected through a native picker and opened as retained read-only regular-file descriptors without following a final symlink. Imported content and job data are encrypted, while app binaries and configuration require normal code-signing protection.
8. **Build pipeline:** npm, Cargo, compiler, and native build dependencies are untrusted supply-chain inputs. Portable verification runs in a digest-pinned, non-root BuildKit container with a credential-free context and no network during project-controlled commands. Native macOS packaging runs afterward on an ephemeral, least-privilege GitHub runner because Apple tooling cannot execute in Linux containers.

## Enforced invariants

- Only backend-created plans can execute; the ID and fingerprint must match.
- Every destructive action requires a grant bound to its plan fingerprint; grants expire after 60 seconds and are consumed once.
- A frozen plan cannot start more than one job.
- Selected message properties are fetched again just before each deletion and again following a rate-limit delay.
- Only `DeletionReach::Everyone` IDs enter a revoke batch.
- A batch contains one chat and no more than 100 message IDs.
- No everyone-scoped plan can fall back to self-only deletion. Self-only chat removal is a separate operation with its own capability check, immutable plan, and confirmation copy.
- Chat-wide operations re-resolve the immutable chat ID and current capability immediately before the call.
- Cancelling prevents later calls; a currently in-flight TDLib call may still complete.
- Frozen message-ID jobs can resume from durable batch progress. Dynamic whole-history, self-only history, sender-wide, and permanent group-deletion jobs are stopped after an ambiguous restart and require a new review so later messages cannot enter the old authorization.
- Test-DC and production plans/databases use separate app-data profiles, and AES-GCM associated data binds persisted jobs to the selected profile. Legacy unbound nonterminal jobs are stopped rather than resumed. The synthetic gateway is test-only, and the production frontend boundary excludes fixture modules.
- Stored plans intentionally retain only IDs, reach expectations, titles needed for confirmation, counters, and timestamps—not message text or media.
- Discord archive search does not require a live token. Remediation requires a verified token whose `/users/@me` ID exactly matches the selected archive owner; a live response cannot add targets beyond exact owner-authored archive locators.
- Discord tokens are memory-only by default. Keychain persistence is explicit opt-in; forget and shutdown clear the in-process value, and no credential is returned to the webview or stored in a plan/job.
- Automatic browser capture never reads an existing profile. Only a bounded non-Bot/non-Bearer `Authorization` value observed on an HTTPS `discord.com/api` request is accepted; cookies, passwords, response bodies, other headers, and protocol events are not retained.
- Discord deletion serializes each channel, limits cross-channel concurrency, paces the account, honors bounded server rate limits, and retries network/5xx ambiguity only a bounded number of times. Exhaustion is uncertain, never confirmed.

## Encrypted archive backend boundary

The separate archive backend stores only explicitly imported normalized observations. The production Discord importer accepts one bounded, versioned Data Package profile and exposes search/remediation through the same scoped provider contract; X and other archive providers remain unavailable. Its SQLCipher/OpenSSL source provenance, crypto flags and licenses are recorded in [ARCHIVE_STORAGE.md](ARCHIVE_STORAGE.md) and the pinned native provenance manifest. Archive evidence and inert locators alone never grant remote remediation authority.

Every import/query/removal binds provider, account and source. Backend-issued sessions and committed checkpoints control mutation; published generations bind query cursors. A bounded blocking worker rejects oversized requests before copying/queueing, allows two pending batches, and observes a shared cancellation signal before queued mutation and commit. Source removal preserves surviving observations and IDs, retires old authority and records pending maintenance durably. Memory-only identity pruning and `VACUUM` can require substantial RAM; memory/disk failure does not permit plaintext temporary storage or restoring logically removed content.

One macOS credential lease protects the consolidated cached vault across Telegram and archive consumers in cooperating processes. It is acquired lazily at the explicitly bound application root, fail-fast before credential I/O, and retained until all tracked archive workers drain and secrets are cleared. Clearing permanently rejects later credential I/O. Symlinked/nonregular/shared-permission lock files fail closed. Store opening obtains its own lock before entering the vault; existing settings-first leases are reused and the vault never calls back into a store.

An already-running older binary does not honor this lease: users must quit all old copies before first archive use. Archive-key creation and optional Discord-token persistence lazily upgrade the same versioned item without changing existing Telegram values; unknown versions fail closed and vault-format downgrades are unsupported. The lease does not defend against malicious same-user software bypassing it. SQLCipher and secure-delete provide no forensic-erasure promise for SSDs, snapshots, backups, exports or remote copies. Final-identity native Keychain prompts/ACLs and plaintext/privacy inspection remain explicit manual release gates.

## Threats and mitigations

| Threat | Mitigation | Residual risk |
| --- | --- | --- |
| Compromised webview invokes IPC directly | Backend-created fingerprinted plans, single-use macOS owner authentication for every destructive plan, and a backend-derived native prompt identifying the immutable target | Malware controlling the user session may also control the native process |
| Permission changes after preview | Per-message and chat capability recheck at action time | Telegram can change state during an in-flight request |
| Partial batch failure or process crash | Encrypted durable cursor, bounded batches, idempotent rechecks, normalized partial status | A response lost after Telegram commits can make local counters conservative |
| Telegram rate limiting | Parse flood waits, persist queued state, remain cancellable, then recheck and retry | Very long or repeated server waits delay completion |
| Discord token captured from the wrong account | Verify the token with `/users/@me` and require an exact match to the imported archive owner before plans are available | A stolen matching token still carries that user's authority until invalidated |
| Lookalike origin or unrelated browser header is captured | Accept only HTTPS Discord API origins and a bounded plausible user `Authorization` value; reject cookies, Bot/Bearer values, controls, redirects and other hosts | A compromised browser or local process can forge protocol events or observe the token |
| Existing browser profile or password store is exposed | Launch a new temporary profile; never inspect the user's normal profile; always offer manual entry | The temporary profile can contain a Discord session until best-effort cleanup succeeds; a browser crash or hostile same-user process can leave or read it |
| Loopback automation endpoint is reached by another local process | Bind only to loopback, use an ephemeral port/profile, keep the capture window bounded, and validate every accepted event and final Discord identity | Another process running as the user may race or interfere with browser automation |
| Discord rate-limit abuse or ambiguous deletion | Conservative per-channel/account pacing, four-channel bound, bounded `retry_after`, cancellation checks, bounded retries, and uncertain terminal outcomes | Unsupported automation can still trigger Discord enforcement; an in-flight request may complete after cancellation |
| Archive and live Discord accounts disagree | Provider/account/source-scoped opaque refs plus exact archive-owner/live-user match; live API responses cannot widen the frozen target list | A stale archive may list messages the account can no longer access |
| Wrong group destroyed | Separate critical UI, immutable chat ID, exact title, irreversible acknowledgement, and a native system prompt containing the sanitized authoritative title, chat ID, and plan token | Similar Unicode chat titles can still confuse a user; the numeric ID and plan token disambiguate the target |
| Secret leakage from logs | No message bodies in jobs, zeroized in-memory credentials, frontend never receives API secrets | TDLib and OS diagnostic logs are separate components and need release review |
| Tampered, rolled-back, or transplanted job file | Profile-bound AES-GCM authentication; malformed or cross-profile stores fail closed; non-idempotent broad jobs never auto-replay after restart | Deleting the store loses local progress but cannot create Telegram authority |
| Malicious TDLib library path | Bundled artifact from an exact source commit, build-time SHA-256 check, loaded-version check, and ad-hoc app-bundle signature | Developer overrides intentionally permit a custom path; unsigned previews lack Apple notarization, so checksum/provenance verification or a source build remains necessary |
| Compromised build dependency targets the developer host | Integrity-checked lockfiles, npm lifecycle scripts disabled during install, non-root container user, secrets and local state excluded from context, no Docker socket mount, and network disabled for build/test commands | Docker daemon/base images and online dependency acquisition remain trusted; malicious code can still corrupt the produced artifact |
| Compromised GitHub Action or overprivileged workflow | Official actions pinned to full commit SHAs, read-only repository permission, checkout credentials removed, no Telegram/release secrets, and native packaging gated on both container architectures | GitHub-hosted runner images and the pinned action commits remain trusted; release signing will require a separately protected workflow |

## Privacy limits

A screenshot, photo, document, caption, or other media attached to a Telegram message is deleted with that message when Telegram accepts an everyone-scoped deletion. A Telegram deletion is not a global erasure primitive: separately saved screenshots/files, exports, forwards, quotes, push-notification history, search-engine caches, device backups, moderation logs, or databases outside Telegram may survive. Secret-chat availability is limited to content known by the local TDLib session/device. Retract must describe Telegram’s accepted result precisely and avoid claims such as “removed from the internet” or “zero trace.”

Discord cleanup deletes only exact owner-authored message IDs present in the selected archive. It cannot delete other users' messages, discover messages missing from the package, recover access to departed/private spaces, or guarantee immediate attachment-cache removal. Discord may retain some data for legal, safety, backup, or policy reasons, and copied/quoted/downloaded content can survive. The imported package is historical evidence and remains on disk until separately removed.

Sensitive-data detection is heuristic. It can miss novel formats, context-dependent identifiers, personal names, text embedded in media pixels, and document bodies, and it can produce false positives. Users must review every result. A full privacy scan may ask TDLib to page through substantial history; message bodies remain in memory only as needed for matching and are not written to Retract’s job log, but TDLib’s encrypted local database remains part of the trusted local boundary.

## Preview residual risks and future release hardening

- Independently reproduce the pinned TDLib build in CI and compare the reviewed artifact/provenance.
- Preview archives use hardened runtime and an ad-hoc signature but are not Apple-notarized. A future officially signed distribution should test Keychain behavior under its final application identity, notarize, and staple the build.
- Review TDLib logging configuration and verify no content-bearing logs persist.
- Add a dependency/SBOM and native-library vulnerability scan to CI.
- Run the entire test-DC matrix and an independent destructive-action security review.
