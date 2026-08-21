# Threat model

## Objective

Retract helps the signed-in Telegram user revoke history that Telegram still permits them to delete for all participants. The primary security property is scope integrity: the app must perform only the operation the user reviewed, against immutable IDs, and must never quietly weaken “for everyone” into “for me.”

## Protected assets

- Telegram authorization/session state and API credentials.
- The TDLib database encryption key and encrypted job-store key.
- Private message metadata shown in memory.
- Frozen deletion plans and their execution status.
- The user’s ability to distinguish owner/admin/member authority and deletion reach.

## Trust boundaries

1. **React webview:** untrusted for destructive authorization. It displays plans and gathers intent but cannot mint a valid backend plan or system-authentication grant.
2. **Rust core:** trusted enforcement boundary. It validates IDs and capabilities, freezes plans, checks confirmation proofs, consumes system grants, persists jobs, and calls the gateway.
3. **TDLib dynamic library:** trusted native dependency pinned by version and exact release commit. The normal consumer build bundles it as an app resource; the build verifies its recorded SHA-256 and the adapter verifies the loaded version. A substituted library still executes inside the application process, so users must verify preview provenance or build from reviewed source.
4. **Telegram service:** authoritative for current permissions and deletion outcomes. Capabilities may change between preview and execution.
5. **macOS Keychain and LocalAuthentication:** trusted for secret storage and fresh device-owner verification.
6. **Local filesystem:** considered observable or modifiable by other software running as the user; confidential job data is authenticated encryption, while app binaries and configuration require normal code-signing protection.
7. **Build pipeline:** npm, Cargo, compiler, and native build dependencies are untrusted supply-chain inputs. Portable verification runs in a digest-pinned, non-root BuildKit container with a credential-free context and no network during project-controlled commands. Native macOS packaging runs afterward on an ephemeral, least-privilege GitHub runner because Apple tooling cannot execute in Linux containers.

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

## Threats and mitigations

| Threat | Mitigation | Residual risk |
| --- | --- | --- |
| Compromised webview invokes IPC directly | Backend-created fingerprinted plans, single-use macOS owner authentication for every destructive plan, and a backend-derived native prompt identifying the immutable target | Malware controlling the user session may also control the native process |
| Permission changes after preview | Per-message and chat capability recheck at action time | Telegram can change state during an in-flight request |
| Partial batch failure or process crash | Encrypted durable cursor, bounded batches, idempotent rechecks, normalized partial status | A response lost after Telegram commits can make local counters conservative |
| Telegram rate limiting | Parse flood waits, persist queued state, remain cancellable, then recheck and retry | Very long or repeated server waits delay completion |
| Wrong group destroyed | Separate critical UI, immutable chat ID, exact title, irreversible acknowledgement, and a native system prompt containing the sanitized authoritative title, chat ID, and plan token | Similar Unicode chat titles can still confuse a user; the numeric ID and plan token disambiguate the target |
| Secret leakage from logs | No message bodies in jobs, zeroized in-memory credentials, frontend never receives API secrets | TDLib and OS diagnostic logs are separate components and need release review |
| Tampered, rolled-back, or transplanted job file | Profile-bound AES-GCM authentication; malformed or cross-profile stores fail closed; non-idempotent broad jobs never auto-replay after restart | Deleting the store loses local progress but cannot create Telegram authority |
| Malicious TDLib library path | Bundled artifact from an exact source commit, build-time SHA-256 check, loaded-version check, and ad-hoc app-bundle signature | Developer overrides intentionally permit a custom path; unsigned previews lack Apple notarization, so checksum/provenance verification or a source build remains necessary |
| Compromised build dependency targets the developer host | Integrity-checked lockfiles, npm lifecycle scripts disabled during install, non-root container user, secrets and local state excluded from context, no Docker socket mount, and network disabled for build/test commands | Docker daemon/base images and online dependency acquisition remain trusted; malicious code can still corrupt the produced artifact |
| Compromised GitHub Action or overprivileged workflow | Official actions pinned to full commit SHAs, read-only repository permission, checkout credentials removed, no Telegram/release secrets, and native packaging gated on both container architectures | GitHub-hosted runner images and the pinned action commits remain trusted; release signing will require a separately protected workflow |

## Privacy limits

A screenshot, photo, document, caption, or other media attached to a Telegram message is deleted with that message when Telegram accepts an everyone-scoped deletion. A Telegram deletion is not a global erasure primitive: separately saved screenshots/files, exports, forwards, quotes, push-notification history, search-engine caches, device backups, moderation logs, or databases outside Telegram may survive. Secret-chat availability is limited to content known by the local TDLib session/device. Retract must describe Telegram’s accepted result precisely and avoid claims such as “removed from the internet” or “zero trace.”

Sensitive-data detection is heuristic. It can miss novel formats, context-dependent identifiers, personal names, text embedded in media pixels, and document bodies, and it can produce false positives. Users must review every result. A full privacy scan may ask TDLib to page through substantial history; message bodies remain in memory only as needed for matching and are not written to Retract’s job log, but TDLib’s encrypted local database remains part of the trusted local boundary.

## Preview residual risks and future release hardening

- Independently reproduce the pinned TDLib build in CI and compare the reviewed artifact/provenance.
- Preview archives use hardened runtime and an ad-hoc signature but are not Apple-notarized. A future officially signed distribution should test Keychain behavior under its final application identity, notarize, and staple the build.
- Review TDLib logging configuration and verify no content-bearing logs persist.
- Add a dependency/SBOM and native-library vulnerability scan to CI.
- Run the entire test-DC matrix and an independent destructive-action security review.
