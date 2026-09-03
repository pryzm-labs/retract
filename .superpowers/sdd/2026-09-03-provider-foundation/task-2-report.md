# Task 2 implementation report

Status: DONE

Base for review: `cee589cc2b41224ccaae0b9cce9837f36454feb2`

Implementation parent at commit time also contains controller-only clarifications `10813cb` and `e10dc22`; neither changed Task 2 implementation files.

## Delivered

- Added the pure `retract-domain` crate. It has no Tauri, filesystem, network, provider adapter, or `cleaner-domain` dependency.
- Added validated `ProviderKey`, distinct UUID-backed account/source/conversation/content/actor/grouping identities, `Scope`, `ActiveContext`, versioned payloads, resource kinds, provider refs, and scoped refs. UUID JSON is string-only; nil application IDs are invalid.
- Implemented the required standard UUIDv5 resource rule exactly: account UUID namespace plus compact JSON tuple `["retract-resource-v1", resourceKind, locatorSchema, locatorVersion, canonicalKey]`. The two literal message strings `9007199254740992` and `9007199254740993` derive distinct IDs with no numeric conversion. All existing Task 1 resource fixtures, including the negative chat ID and `message:part/0007`, match their independent literal UUIDs.
- Added provider-neutral account/source, actor/conversation/content and inert-attachment records, all normalized content/conversation/evidence taxonomies, privacy-kind values matching the existing detector, and opaque `VersionedPayload` provider metadata. Typed Telegram metadata deliberately remains outside this crate; the tests prove outgoing/pinned/grouping/original-kind data survives opaque round trips.
- Added all nine action kinds, seven effects, four availability values, action advisories/batch constraints, confirmation tiers and safety invariants. Container destruction is always critical.
- Added separately typed provider and application errors for every approved variant. DTO serialization derives predefined safe messages from enum codes, rejects unknown/native/details bags and message/code disagreement, and requires `rate_limited` (only that provider error) to carry `retryAt`.
- Added `RemediationPlan::{seal,validate}` with recursively key-ordered canonical JSON, UUID+schema-version SHA-256 binding, unordered target sort/dedup, conflicting same-ID reference rejection, preserved ordered steps, exact scope/reference checks, confirmation checks and conservative restart policy. Recipe payloads containing a nested `fingerprint` are rejected so the binding excludes its own digest by construction.
- Added scoped jobs with blocked state, frozen dirty refs, durable `startedAuthorized` provenance, retry/timestamps, safe diagnostics and separate selected/eligible/deleted/skipped/failed/uncertain counters. Neutral result accounting never adds `skipped` to eligible results. Added an explicitly unbound/non-executable `LegacyHistoryRecord` that retains legacy operation/counters while accepting no scope, recipe, retry or nonterminal status.
- Added Docker locked fetch/test/fmt/Clippy coverage for the new crate, `npm run test:release` to the standard Docker checks stage, npm full-check coverage, Dependabot, public MIT/version metadata, and release version-update coverage against temporary fixture repositories.
- Generated both Cargo locks inside the pinned Docker toolchain. The `src-tauri` lock only adds `retract-domain` and UUID-v5's `sha1_smol 1.0.1`; unrelated versions are unchanged. The crate lock is independent because the repository has no Cargo workspace.

## Interfaces and consumer obligations

- `ProviderResourceRef::resource_id()` validates only the neutral bounded envelope and derives its UUID. A provider adapter must still deserialize and validate its typed schema/version/payload/native ranges and canonical-key agreement before publishing or executing the reference.
- `ScopedResourceRef::validate(expected_scope)` enforces exact scope, embedded provider/account, and derived ID. `expected_scope` must come from a durably verified account/source relationship; calling it with the ref's own untrusted scope does not prove source ownership.
- `SourceRecord::validate(account)` enforces provider/account ownership and live-vs-archive provenance. Task 3 should validate account/source relationships before validating stored plans/jobs; Task 4 owns authenticated Telegram identity and typed locator semantics.
- `RemediationPlan::seal()` canonicalizes target sets in both the global effect-target list and ordered steps, mutates the plan to that canonical representation, then stores `sha256-v1:<64 lowercase hex>`. `validate()` validates a canonical clone and recomputes without mutating loaded state. Adapters must additionally validate supported recipe schema and exact typed recipe/target semantics.
- Global plan `targets` are actual effect targets. Reviewed unavailable/protected selection evidence is not converted into executable steps; it remains adapter-owned recipe/summary evidence. Step order remains execution order.
- `ScopedJobRecord::validate(plan)` requires the plan to validate, binds plan/scope, validates dirty refs, blocked diagnostics, timestamps/provenance, and normalized result limits. It intentionally does not infer provider batch counts from `nextBatch`; Task 3/5 must validate typed cursors and restart eligibility before recovery.
- `JobCounters` defines `eligible` as executable targets and `selected` as all reviewed selections. It requires `eligible <= selected`, `skipped <= selected`, and `deleted + failed + uncertain <= eligible`; skipped is not included in that sum. `LegacyHistoryRecord` imposes no arithmetic relationship, preserving valid legacy rows where deleted+skipped+failed can exceed the legacy eligible total.
- `SafeError` is the durable/job diagnostic union. `ProviderError` and `ApplicationError` are separate boundary DTO taxonomies. All are content-free; no raw error string can be deserialized into them.

## TDD RED evidence

Initial domain contracts, before implementation:

```sh
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=cargo test --offline --locked --manifest-path crates/retract-domain/Cargo.toml; npm run test:release' --output type=cacheonly --progress plain .
```

Build `s95yqz2zyv4d1i5qab4g00ik6`, exit 1. The new crate compiled, then all 11 initial behavior tests failed for their intended missing behavior: provider validation, safe messages, nonnil UUIDs, envelope bounds, literal UUIDv5, scope/reference checks, restart policy, SHA-256 binding, conflicting refs, scoped job validation and target canonicalization. Actual summary:

```text
running 11 tests
test result: FAILED. 0 passed; 11 failed; 0 ignored
```

Release integration before the fifth manifest was consumed:

```sh
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=npm run test:release' --output type=cacheonly --progress plain .
```

Build `vyl0tu7438yijnjjjotou08mx`, exit 1. Six preserved release tests passed; the temporary-repository neutral-crate version mismatch was not detected:

```text
tests 7
pass 6
fail 1
AssertionError [ERR_ASSERTION]: Missing expected exception.
```

Later focused RED cycles were run before their corresponding production changes:

```text
normalized records/safe serialization: 10 passed, 3 failed (foreign content scope, account/source ownership, safe message serialization)
conversation validation: 14 passed, 1 failed (wrong provider metadata version)
legacy_history: unknown field `operation`
provider_and_application_errors: types not found, then retry-limited invariant failed
descriptor container destruction: expected descriptor.validate().is_err()
scoped counter accounting: expected job.validate(&plan).is_err()
```

An independent one-use Node `crypto` calculation over a hand-built, recursively sorted binding produced literal `sha256-v1:515df8e9f662a144fa0861b98910151d5dd1c2ed346485b92be3249ba299d48d`; the Rust contract test freezes this value and uses no production fingerprint helper for the expectation.

## GREEN verification

Fresh final focused gate after all implementation changes:

```sh
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=cargo fmt --manifest-path crates/retract-domain/Cargo.toml -- --check && cargo test --offline --locked --manifest-path crates/retract-domain/Cargo.toml && cargo clippy --offline --locked --manifest-path crates/retract-domain/Cargo.toml --all-targets -- -D warnings && cargo clippy --offline --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && npm run test:release && npm run check:public-repo' --output type=cacheonly --progress plain .
```

Build `i50jgjclvtntidewpem9me512`, exit 0:

```text
retract-domain: 18 passed, 0 failed; doc tests 0 failed
retract-domain Clippy: exit 0, -D warnings
src-tauri dependent Clippy: exit 0, -D warnings
release metadata: 7 passed, 0 failed
Public repository metadata and documentation are consistent.
```

The preceding full compatibility gate after implementation also exited 0 as build `qmlhoavrozvqwmqm8dc0qgfiq`:

```text
fmt --check: cleaner-domain, retract-domain, src-tauri all pass
retract-domain: 18 passed; cleaner-domain: 17 passed; src-tauri preserved suite: 63 passed
Clippy -D warnings: all three manifests pass
release: 7 passed; App/IPC controls: 23 passed
public metadata and production-bundle checks pass
```

The 11 Task 1 backend REDs were intentionally selected out of the compatibility gate; no test is ignored in source.

## Intentional lifecycle RED preservation

```sh
docker buildx build --target focused-checks --build-arg 'RETRACT_CHECK=set +e; npm test -- src/provider-lifecycle.test.tsx; frontend=$?; cargo test --offline --locked --manifest-path src-tauri/Cargo.toml foundation_lifecycle; backend=$?; echo EXPECTED_RED_EXIT_CODES frontend=$frontend backend=$backend; test $frontend -eq 1 && test $backend -eq 101' --output type=cacheonly --progress plain .
```

Build `d6l1ayeau04dzdoj0w82mhq2i`, wrapper exit 0 after confirming the exact intentional failures:

```text
frontend: 8 failed, 3 controls passed (11 total), exit 1
backend: 11 failed, 0 passed, 63 filtered out, exit 101
EXPECTED_RED_EXIT_CODES frontend=1 backend=101
```

These are the expected Tasks 4–7 production lifecycle gaps. Task 2 introduces neutral IDs but does not migrate legacy IPC, UI, encrypted store or executor behavior.

## Files

Created:

- `crates/retract-domain/Cargo.toml`
- `crates/retract-domain/Cargo.lock`
- `crates/retract-domain/src/lib.rs`
- `crates/retract-domain/src/identity.rs`
- `crates/retract-domain/src/records.rs`
- `crates/retract-domain/src/actions.rs`
- `crates/retract-domain/src/plans.rs`
- `crates/retract-domain/src/error.rs`
- `crates/retract-domain/tests/contracts.rs`
- `.superpowers/sdd/2026-09-03-provider-foundation/task-2-report.md`

Modified:

- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`
- `Dockerfile`
- `package.json`
- `.github/dependabot.yml`
- `scripts/check-public-repo.mjs`
- `scripts/release-metadata.mjs`
- `scripts/release-metadata.node-test.mjs`

## Self-review and concerns

- `git diff --check` is clean. No task-unrelated implementation file changed; controller ledger/progress files were not edited.
- Canonical UUID and plan fingerprints have independent literal expectations. Tests mutate every fingerprint-bound field category, reverse step order, reorder/deduplicate target sets and object keys, and create same-ID conflicting envelopes.
- The explicit controller request not to spawn reviewers overrides the normally applicable review skill; independent review remains controller-owned.
- No real Telegram, keychain, network provider, destructive action, Docker prune, artifact image export, merge, push or release was performed.
- The two failed direct `docker run` formatting attempts made no source changes; all successful formatting and verification ran in pinned build stages, offline and nonroot with named caches/cacheonly output.
- No unresolved Task 2 implementation concern. Provider-owned locator/recipe/cursor validation and final lifecycle GREEN are deliberately assigned to later tasks and are called out above rather than silently inferred by the neutral crate.
