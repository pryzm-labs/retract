# Retract Open-Source Preview Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish-ready Retract source with real Telegram onboarding, development-only synthetic fixtures, public project documentation, and reproducible unsigned Apple-silicon preview releases.

**Architecture:** Production frontend builds resolve only a desktop Tauri API adapter, while Vitest and the dedicated screenshot mode resolve a fixture adapter. The native application replaces its fixture fallback with an inert setup gateway and retains the Rust fixture gateway only under `cfg(test)`. A local packaging script and least-privilege tag workflow build, validate, archive, checksum, attest, and publish an ad-hoc-signed arm64 application.

**Tech Stack:** React 19, TypeScript 5.9, Vite 7, Vitest 3, Tauri 2, Rust 1.97.1, TDLib 1.8.64, Docker BuildKit, GitHub Actions, POSIX shell, Node.js 24.

**Spec:** `docs/superpowers/specs/2026-08-20-open-source-preview-release-design.md`

## Global Constraints

- License all original Retract source under MIT with `Copyright (c) 2026 Pryzm Labs`.
- Support only Apple-silicon Macs running macOS 12 or later in v0.1.0.
- Do not add Apple Developer signing, notarization, provisioning, or Apple secrets.
- Production onboarding offers only real Telegram connection configured through the UI.
- Synthetic fixtures are available only to automated tests and `vite --mode screenshot`.
- Production code never falls back to fixtures when Telegram configuration or startup fails.
- Every preview archive is ad-hoc signed, explicitly unsigned/unnotarized, checksummed, traceable to a source tag, and published as a GitHub pre-release.
- Never include Telegram credentials, sessions, databases, job state, real chat screenshots, Docker caches, or local build output.
- Do not advise users to disable Gatekeeper globally or broadly clear quarantine metadata.
- Preserve the existing deletion invariant: an everyone-scoped operation never falls back to self-only deletion.

---

### Task 1: Audit and import the existing application baseline

**Files:**
- Modify: `.gitignore`
- Existing source import: all current non-ignored project files except the already committed design specification

**Interfaces:**
- Consumes: the user-initialized repository and design commit `fc57669`
- Produces: a complete, reviewable baseline commit on branch `main` for subsequent task diffs

- [ ] **Step 1: Run the repository security and private-artifact audits before staging source**

Run the standard Codex repository security scan using `codex-security:security-scan`. Separately list only filenames—not matching values—for private-key, credential, `.env`, Telegram database, and session patterns while excluding ignored dependency/build trees.

Expected: no real credential/session files are eligible for staging; validated high-impact security findings are resolved before the baseline source commit.

- [ ] **Step 2: Make worktree isolation explicit and ignore it**

Add the following entry to `.gitignore`:

```gitignore
.worktrees/
```

Run:

```bash
git check-ignore -q .worktrees
```

Expected: exit 0.

- [ ] **Step 3: Rename the initial branch and stage the baseline**

Run:

```bash
git branch -m main
git add .
git status --short
```

Expected: tracked source includes `vendor/tdlib-dist/libtdjson.dylib`, its license/stamp, locks, docs, icons, and workflows; it excludes `.DS_Store`, `.env`, `node_modules`, `dist`, `artifacts`, both Rust target trees, `src-tauri/tdlib-data`, `src-tauri/gen`, and `vendor/tdlib-source`.

- [ ] **Step 4: Verify the staged tree is publishable**

Run:

```bash
git diff --cached --stat
git ls-files
```

Manually confirm no path matches the exclusions above and no tracked file exceeds GitHub's 100 MB file limit. Confirm the 26 MB TDLib dylib is arm64, has only macOS system-library runtime dependencies, and matches its recorded SHA-256.

- [ ] **Step 5: Commit the existing baseline**

```bash
git commit -m "chore: import Retract application baseline"
```

### Task 2: Make real Telegram setup the only production onboarding

**Files:**
- Modify: `src/components/ConnectionSettingsDialog.test.tsx`
- Modify: `src/components/ConnectionSettingsDialog.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/components/Sidebar.tsx`
- Modify: `src/components/ImpactPanel.tsx`
- Modify: `src/types.ts`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: `ConnectionSettings`, `SaveConnectionSettingsRequest`, and `SaveConnectionSettingsResult` from `src/types.ts`
- Produces: `SaveConnectionSettingsRequest { tdlibPath, apiId, apiHash, useTestDc }` with no runtime selector; `ConnectionSettingsDialog` that always renders live Telegram fields

- [ ] **Step 1: Write the failing connection-dialog tests**

Replace the Safe demo test with these behaviors:

```tsx
it("requires real Telegram credentials without offering fixture mode", () => {
  render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={vi.fn()} />);

  expect(screen.getByRole("heading", { name: "Connect Telegram" })).toBeInTheDocument();
  expect(screen.queryByText("Safe demo")).not.toBeInTheDocument();
  expect(screen.getByLabelText("Telegram API ID")).toBeInTheDocument();
  expect(screen.getByLabelText("Telegram API hash")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Save settings" })).toBeDisabled();
});

it("saves only real Telegram connection fields", async () => {
  const save = vi.spyOn(api, "saveConnectionSettings").mockResolvedValue(liveResult);
  render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={vi.fn()} />);

  fireEvent.change(screen.getByLabelText("Telegram API ID"), { target: { value: "12345678" } });
  fireEvent.change(screen.getByLabelText("Telegram API hash"), { target: { value: "0123456789abcdef0123456789abcdef" } });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));

  await waitFor(() => expect(save).toHaveBeenCalledWith({
    tdlibPath: bundledSettings.tdlibPath,
    apiId: 12345678,
    apiHash: "0123456789abcdef0123456789abcdef",
    useTestDc: false
  }));
});
```

The production change these tests catch is reintroducing a fixture selector or sending a mode field that the backend could use to activate it.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
npm test -- src/components/ConnectionSettingsDialog.test.tsx
```

Expected: FAIL because the current dialog headings/mode buttons and request still include Safe demo/runtime mode.

- [ ] **Step 3: Implement the single-mode connection dialog and request type**

Remove `RuntimePreference` from the connection request/view types, delete the mode state/buttons and demo note, render the Telegram fields unconditionally, and compute readiness as:

```ts
const hashReady = settings.apiHashConfigured || /^[a-fA-F0-9]{32}$/.test(apiHash.trim());
const saveReady = Boolean(tdlibPath.trim() && Number(apiId) > 0 && hashReady);
```

Use first-run copy `Connect Telegram` and settings copy `Telegram connection`. Change “For this development build” to “Obtain both values from my.telegram.org → API development tools.”

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
npm test -- src/components/ConnectionSettingsDialog.test.tsx
```

Expected: both dialog tests PASS.

- [ ] **Step 5: Write the failing application-shell test**

Add:

```tsx
it("never exposes fixture controls in the end-user shell", async () => {
  render(<App />);
  expect(await screen.findByText("Search every chat")).toBeInTheDocument();
  expect(screen.queryByText(/Safe demo/i)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Reset demo fixtures/i })).not.toBeInTheDocument();
});
```

Expected production mutation caught: restoring the demo banner, runtime pill, or reset control.

- [ ] **Step 6: Run the application test and verify RED**

Run:

```bash
npm test -- src/App.test.tsx
```

Expected: FAIL because the current fixture shell exposes the Safe demo banner and reset control.

- [ ] **Step 7: Remove user-facing demo chrome and props**

Delete `resetDemo` from `App`, the demo banner, `FlaskConical` import, `runtimeMode`/`onResetDemo` from `ImpactPanel`, and all demo branches from `Sidebar`. The public shell uses a static `TELEGRAM` service pill and “Telegram account” subtitle, so screenshot fixtures do not claim a separate Retract service.

Keep `AppSnapshot.runtimeMode` temporarily because native/test gateways still serialize it during Task 3; do not use it to expose fixture controls.

- [ ] **Step 8: Run frontend tests and commit**

```bash
npm test
git add src/components/ConnectionSettingsDialog.test.tsx src/components/ConnectionSettingsDialog.tsx src/App.test.tsx src/App.tsx src/components/Sidebar.tsx src/components/ImpactPanel.tsx src/types.ts src/styles.css
git commit -m "feat: require Telegram setup in the public UI"
```

Expected: all frontend tests PASS.

### Task 3: Remove the native production fixture fallback

**Files:**
- Create: `src-tauri/src/setup_gateway.rs`
- Modify: `src-tauri/src/connection_settings.rs`
- Modify: `src-tauri/src/gateway.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/service.rs`
- Modify: `src-tauri/src/secure_store.rs`
- Modify: `src-tauri/src/live_gateway.rs`
- Modify: `src-tauri/src/demo_gateway.rs`

**Interfaces:**
- Consumes: `TelegramGateway`, `GatewayInfo`, `AuthSnapshot`, and `CleanerService`
- Produces: `SetupGateway::new(reason: impl Into<String>) -> Arc<SetupGateway>`; setup snapshots identify as live, contain no chats, and expose `AuthStage::Error` with the setup reason

- [ ] **Step 1: Write failing migration and setup-gateway tests**

In `connection_settings.rs`, add a test that deserializes schema-1 demo settings and asserts the public view helper requires setup:

```rust
#[test]
fn legacy_demo_settings_are_treated_as_unconfigured() {
    let stored: StoredConnectionSettings = serde_json::from_str(
        r#"{"schemaVersion":1,"setupComplete":true,"runtimeMode":"demo"}"#,
    ).unwrap();
    assert!(!public_setup_complete(&stored));
}
```

In `setup_gateway.rs`, add an async test asserting `info().mode == "live"`, `chats()` is empty, `auth().stage == AuthStage::Error`, and `delete_group(7)` returns `InvalidRequest` containing “configure Telegram”.

The mutations caught are trusting persisted demo mode or accidentally granting a destructive method on the inert gateway.

- [ ] **Step 2: Run backend tests and verify RED**

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml legacy_demo_settings_are_treated_as_unconfigured
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml setup_gateway
```

Expected: compilation/test failure because `public_setup_complete` and `SetupGateway` do not exist.

- [ ] **Step 3: Make stored mode migration-only and live-only on save**

Keep `RuntimePreference::{Demo, Live}` private so schema-1 files deserialize safely, but default to `Live`. Remove `runtime_mode` from `ConnectionSettingsView` and `SaveConnectionSettingsRequest`. Save `RuntimePreference::Live` unconditionally. Implement:

```rust
fn public_setup_complete(settings: &StoredConnectionSettings) -> bool {
    settings.setup_complete && settings.runtime_mode == RuntimePreference::Live
}
```

`get_view` uses this helper. `effective_live` returns `None` for legacy demo data, causing setup rather than fixture execution.

- [ ] **Step 4: Implement the inert setup gateway**

`SetupGateway` returns:

```rust
GatewayInfo {
    mode: "live",
    account_label: "Telegram setup required".into(),
    reason: Some(self.reason.clone()),
}
```

It returns empty chats/search results only for read-only shell bootstrap, returns `AuthStage::Error` with the reason, and returns the same `InvalidRequest("configure Telegram before using Retract")` for authentication/deletion methods. `close()` succeeds.

- [ ] **Step 5: Replace fixture fallback and remove reset IPC**

Compile `demo_gateway` only under `#[cfg(test)]`. `create_service` uses `SetupGateway` for missing or invalid live settings and `SecureJobStore::open_setup(base_data_dir.join("setup"))`; it never constructs `DemoGateway`. Remove `reset_demo` from the Tauri handler, service, trait, and live gateway. Retain fixture reset as an inherent method on test-only `DemoGateway` if Rust tests need it.

- [ ] **Step 6: Run backend tests and verify GREEN**

```bash
RETRACT_TEST_TDLIB_PATH="$PWD/vendor/tdlib-dist/libtdjson.dylib" cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Expected: backend tests and clippy PASS with no production reference to `DemoGateway` or reset IPC.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src
git commit -m "feat: isolate native fixtures from production"
```

### Task 4: Exclude synthetic fixture data from production frontend assets

**Files:**
- Create: `src/api-contract.ts`
- Create: `src/api.desktop.ts`
- Create: `src/api.fixture.ts`
- Create: `scripts/verify-production-bundle.sh`
- Modify: `src/api.ts` (remove after adapter split)
- Modify: `src/App.tsx`
- Modify: `src/components/ConnectionSettingsDialog.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/components/ConnectionSettingsDialog.test.tsx`
- Modify: `src/api.test.ts`
- Modify: `vite.config.ts`
- Modify: `vitest.config.ts`
- Modify: `tsconfig.json`
- Modify: `package.json`

**Interfaces:**
- Consumes: existing `api` method signatures and `src/demo.ts`
- Produces: `RetractApi` interface; `@retract/api` build-time alias; production `api.desktop.ts`; test/screenshot `api.fixture.ts`; `npm run verify:production-bundle`; `npm run screenshot:dev`

- [ ] **Step 1: Add and run the production-bundle boundary check**

Create `scripts/verify-production-bundle.sh` to run a production build and fail if any emitted file contains stable fixture markers such as `Project Cedar launch credentials` or `Disposable fixtures`. It also fails when an emitted asset contains the `reset_demo` IPC command.

Run:

```bash
sh scripts/verify-production-bundle.sh
```

Expected: FAIL against the current statically imported fixture API.

- [ ] **Step 2: Split the API adapters behind a build-time alias**

Define the application-facing method signatures in `RetractApi`. Move Tauri `invoke` behavior into `api.desktop.ts` with no import from `demo.ts`. Move synthetic behavior into `api.fixture.ts`, including `resetFixtures()` for test setup.

Configure Vite:

```ts
export default defineConfig(({ mode }) => ({
  resolve: {
    alias: {
      "@retract/api": fileURLToPath(new URL(
        mode === "screenshot" ? "./src/api.fixture.ts" : "./src/api.desktop.ts",
        import.meta.url
      ))
    }
  }
}));
```

Configure Vitest to resolve `@retract/api` to `src/api.fixture.ts`, and map the TypeScript path to `src/api.desktop.ts` for production type-checking. Application components import only `@retract/api`; fixture behavior tests import `api.fixture.ts` explicitly.

- [ ] **Step 3: Update fixture tests without weakening assertions**

Change test setup from `api.resetDemo()` to `fixtureApi.resetFixtures()`. Keep each existing search/deletion assertion against the real in-memory fixture implementation. Do not assert on mocks except where the existing UI test replaces external Tauri calls.

- [ ] **Step 4: Run frontend tests and production boundary check**

```bash
npm test
sh scripts/verify-production-bundle.sh
```

Expected: frontend tests PASS; production build PASS; emitted assets contain no fixture markers or reset IPC command.

- [ ] **Step 5: Add screenshot-only launch command and commit**

Add:

```json
"screenshot:dev": "vite --mode screenshot",
"verify:production-bundle": "sh scripts/verify-production-bundle.sh"
```

Run `npm run screenshot:dev` long enough to confirm it serves the synthetic UI, then stop it.

```bash
git add src scripts/verify-production-bundle.sh vite.config.ts vitest.config.ts tsconfig.json package.json package-lock.json
git commit -m "build: exclude fixtures from production assets"
```

### Task 5: Add reproducible unsigned macOS packaging

**Files:**
- Create: `scripts/release-metadata.mjs`
- Create: `scripts/release-metadata.test.mjs`
- Create: `scripts/package-unsigned-macos.sh`
- Create: `docs/RELEASING.md`
- Modify: `package.json`
- Modify: `src-tauri/tauri.conf.json`

**Interfaces:**
- Consumes: `package.json`, `src-tauri/tauri.conf.json`, both Cargo manifests, `vendor/tdlib-dist/build-stamp.txt`, built `Retract.app`
- Produces: `artifacts/release/Retract-vX.Y.Z-macos-arm64.app.zip`, `.sha256`, and `.manifest.json`; `npm run package:unsigned`

- [ ] **Step 1: Write failing release-metadata tests**

Use Node's built-in test runner:

```js
test("rejects inconsistent application versions", () => {
  assert.throws(() => assertVersionConsistency({
    packageVersion: "0.1.0",
    tauriVersion: "0.1.1",
    rustVersion: "0.1.0",
    domainVersion: "0.1.0"
  }), /versions must match/);
});

test("build manifest identifies an unsigned arm64 preview", () => {
  assert.deepEqual(buildManifest(fixture), {
    product: "Retract",
    version: "0.1.0",
    sourceCommit: "0123456789abcdef",
    target: "aarch64-apple-darwin",
    minimumMacosVersion: "12.0",
    signing: "ad-hoc",
    notarized: false,
    tdlib: {
      version: "1.8.64",
      commit: "e0943d068ce90b5010f1aea946e6901e25b43bf6",
      architecture: "arm64",
      sha256: "a89780da629bce37eadba34448622141d89f37bb29a28dfe7293496d5b8ea044"
    }
  });
});
```

- [ ] **Step 2: Run metadata tests and verify RED**

```bash
node --test scripts/release-metadata.test.mjs
```

Expected: FAIL because the metadata module does not exist.

- [ ] **Step 3: Implement metadata validation and generation**

Export `parseBuildStamp(text)`, `assertVersionConsistency(versions)`, and `buildManifest(input)`. Its CLI reads the four manifests and TDLib stamp, rejects mismatch or malformed provenance, obtains the source commit from `RETRACT_SOURCE_COMMIT` or `git rev-parse HEAD`, and writes deterministic JSON to the requested path.

- [ ] **Step 4: Run metadata tests and verify GREEN**

```bash
node --test scripts/release-metadata.test.mjs
```

Expected: both tests PASS.

- [ ] **Step 5: Implement the unsigned packaging script**

The shell script uses `set -eu`, requires `uname -s` = `Darwin` and `uname -m` = `arm64`, runs the normal Tauri app build, verifies:

```bash
codesign --verify --deep --strict "$app_path"
codesign -dv --verbose=4 "$app_path" 2>&1
file "$app_path/Contents/MacOS/retract"
otool -L "$app_path/Contents/Resources/lib/libtdjson.dylib"
```

It requires `Signature=adhoc`, hardened runtime flags, an arm64-only executable, the recorded TDLib checksum, and no non-system TDLib runtime dependency. It packages with `ditto -c -k --sequesterRsrc --keepParent`, writes a SHA-256 file, emits the manifest, expands the archive into a fresh temporary directory, and re-verifies the expanded app before moving outputs into `artifacts/release`.

- [ ] **Step 6: Document release procedure and add scripts**

Add:

```json
"test:release": "node --test scripts/release-metadata.test.mjs",
"package:unsigned": "sh scripts/package-unsigned-macos.sh"
```

`docs/RELEASING.md` specifies version alignment, clean checks, annotated `vX.Y.Z` tags, unsigned status, artifact inspection, and GitHub pre-release behavior. Keep `signingIdentity: "-"` and hardened runtime in Tauri configuration.

- [ ] **Step 7: Run release tests and commit**

```bash
npm run test:release
git add scripts package.json package-lock.json src-tauri/tauri.conf.json docs/RELEASING.md
git commit -m "build: add reproducible unsigned macOS packaging"
```

### Task 6: Add public project metadata, policies, and onboarding documentation

**Files:**
- Create: `LICENSE`
- Create: `SECURITY.md`
- Create: `CONTRIBUTING.md`
- Create: `CODE_OF_CONDUCT.md`
- Create: `CHANGELOG.md`
- Create: `THIRD_PARTY_NOTICES.md`
- Create: `.github/ISSUE_TEMPLATE/bug_report.yml`
- Create: `.github/ISSUE_TEMPLATE/feature_request.yml`
- Create: `.github/ISSUE_TEMPLATE/config.yml`
- Create: `.github/PULL_REQUEST_TEMPLATE.md`
- Create: `.github/dependabot.yml`
- Create: `scripts/check-public-repo.mjs`
- Modify: `README.md`
- Modify: `PLAN.md`
- Modify: `package.json`
- Modify: `src-tauri/Cargo.toml`
- Modify: `crates/cleaner-domain/Cargo.toml`
- Modify: `src-tauri/tauri.conf.json`

**Interfaces:**
- Consumes: repository paths, npm/Cargo metadata, approved release copy, existing threat/test documentation
- Produces: MIT/Pryzm Labs public project contract; `npm run check:public-repo`

- [ ] **Step 1: Add a failing public-repository artifact validator**

`scripts/check-public-repo.mjs` verifies required public files exist, every relative Markdown link resolves, package/Cargo licenses are exactly `MIT`, package engines match `.nvmrc`, Tauri/package/Cargo versions match, README references the logo and screenshot, and issue forms contain the instruction not to upload chat exports, credentials, session files, or unredacted conversations.

Run:

```bash
node scripts/check-public-repo.mjs
```

Expected: FAIL listing the missing policy/license/screenshot artifacts and metadata mismatches.

- [ ] **Step 2: Add license, contribution, conduct, security, and third-party policies**

Use the standard MIT text with `Copyright (c) 2026 Pryzm Labs`. Security reports go through GitHub private vulnerability reporting; public issues must never contain Telegram credentials, sessions, exports, databases, or private conversation screenshots. Contribution instructions require Docker checks and forbid destructive testing on valuable accounts.

`THIRD_PARTY_NOTICES.md` records the vendored nanoid MIT license and bundled TDLib Boost Software License 1.0 provenance, linking to the exact tracked license files.

- [ ] **Step 3: Add GitHub community templates and dependency updates**

Add YAML issue forms for reproducible bugs and scoped feature requests, disable blank issues, point security reports to `SECURITY.md`, and add a pull-request checklist for tests, privacy-safe fixtures, deletion-scope copy, lockfiles, and screenshots.

Dependabot runs weekly for npm (`/`), Cargo (`/src-tauri` and `/crates/cleaner-domain`), GitHub Actions (`/`), and Docker (`/`), each with a small open-pull-request limit.

- [ ] **Step 4: Rewrite the README for end users**

Use this public order: centered logo/title/tagline and relative badges; pre-release/unsigned callout; synthetic screenshot; capabilities; deletion limits; unsigned download instructions; build-from-source commands; first Telegram connection; isolated first test; Docker verification; privacy/security architecture; contributing/license/third-party links.

The download instructions use Apple's per-app path: attempt to open Retract, then System Settings → Privacy & Security → Open Anyway. They do not include `spctl --master-disable` or broad `xattr` commands.

The build path is exactly:

```bash
git clone https://github.com/Pryzm-Labs/retract.git
cd retract
npm ci --ignore-scripts
npm run tauri dev
```

It explains that the normal app build includes TDLib and that environment variables are optional developer/CI overrides, not end-user setup.

- [ ] **Step 5: Align project metadata**

Set npm `description`, `license: "MIT"`, `engines.node: "24.19.0"`, Pryzm Labs author, repository/bugs/homepage once the configured remote supplies the exact organization slug, and `private: true`. Change both Cargo manifests from `MIT OR Apache-2.0` to `MIT`, and update public product descriptions without changing identifier `app.retract.cleaner`.

- [ ] **Step 6: Run the validator and commit**

```bash
node scripts/check-public-repo.mjs
npm test
git add LICENSE SECURITY.md CONTRIBUTING.md CODE_OF_CONDUCT.md CHANGELOG.md THIRD_PARTY_NOTICES.md .github README.md PLAN.md package.json package-lock.json src-tauri/Cargo.toml crates/cleaner-domain/Cargo.toml src-tauri/tauri.conf.json scripts/check-public-repo.mjs
git commit -m "docs: prepare Retract for public contribution"
```

Expected: public artifact validator and frontend tests PASS.

### Task 7: Add least-privilege CI and unsigned tag publication

**Files:**
- Modify: `.github/workflows/secure-build.yml`
- Create: `.github/workflows/release.yml`
- Modify: `scripts/check-public-repo.mjs`

**Interfaces:**
- Consumes: `npm run container:check`, `npm run package:unsigned`, release artifacts from Task 5
- Produces: PR/main verification and `v*` pre-release publication with checksum, manifest, and provenance

- [ ] **Step 1: Extend the public-repository validator with workflow invariants and verify RED**

Validate that the release workflow exists, is tag-only, grants read-only permissions by default, grants `contents: write`, `id-token: write`, and `attestations: write` only to the release job, checks out without persisted credentials, calls `npm run package:unsigned`, and creates a pre-release. Validate every `uses:` reference is pinned to a 40-character commit SHA.

Run:

```bash
node scripts/check-public-repo.mjs
```

Expected: FAIL because the tag release workflow is absent.

- [ ] **Step 2: Implement tag checks and native release packaging**

Trigger only on `push.tags: ["v*"]` and `workflow_dispatch`. Run the same amd64/arm64 Docker checks as `secure-build.yml`, then an arm64 `macos-14` package job. The package job verifies `GITHUB_REF_NAME` equals `v` plus all manifest versions, installs npm dependencies with lifecycle scripts disabled, runs `npm run package:unsigned`, and uploads only the three named release files.

- [ ] **Step 3: Add provenance and GitHub pre-release publication**

Use the official GitHub build-provenance action pinned to a reviewed full SHA. Publish with the preinstalled `gh` CLI and the job-scoped token:

```bash
gh release create "$GITHUB_REF_NAME" \
  artifacts/release/Retract-*.app.zip \
  artifacts/release/Retract-*.sha256 \
  artifacts/release/Retract-*.manifest.json \
  --verify-tag --prerelease --generate-notes \
  --title "Retract ${GITHUB_REF_NAME} (unsigned preview)"
```

The release body links to the README install section and states arm64/macOS 12+, ad-hoc signature, no notarization, Gatekeeper warning, source-build alternative, and destructive pre-release status.

- [ ] **Step 4: Validate workflow syntax and policy**

```bash
ruby -e 'require "yaml"; Dir[".github/**/*.yml"].each { |path| YAML.load_file(path); puts path }'
node scripts/check-public-repo.mjs
```

Expected: every YAML file parses; the repository validator PASSes.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows scripts/check-public-repo.mjs
git commit -m "ci: publish unsigned macOS preview releases"
```

### Task 8: Capture the synthetic product screenshot

**Files:**
- Create: `docs/images/retract-overview.png`
- Modify: `README.md`

**Interfaces:**
- Consumes: `npm run screenshot:dev` and fixture-only Vite mode
- Produces: privacy-safe README image showing the finished product shell without demo controls or personal data

- [ ] **Step 1: Launch screenshot mode**

```bash
npm run screenshot:dev
```

Expected: Vite serves the synthetic fixture application at `http://localhost:1420`; no native Telegram session is touched.

- [ ] **Step 2: Capture the final shell at 1320×820**

Use the in-app browser control skill to open the local page, select the synthetic “Design Team” chat, select two fixture messages, and save a 1320×820 PNG to `docs/images/retract-overview.png`. Stop the server after capture.

- [ ] **Step 3: Inspect the image for privacy and presentation**

Use `view_image` and confirm it contains only known fixture names/messages, the real Retract logo, no Safe demo banner/pill/reset control, no Telegram credentials, no device information, and no browser chrome.

- [ ] **Step 4: Add caption and validate README**

Caption the image “Synthetic fixture data shown; no real Telegram account or conversation is pictured.”

```bash
node scripts/check-public-repo.mjs
git add docs/images/retract-overview.png README.md
git commit -m "docs: add privacy-safe Retract screenshot"
```

### Task 9: Complete security, build, and release-artifact verification

**Files:**
- Modify only files required to resolve validated findings or verification failures

**Interfaces:**
- Consumes: the approved spec, all task commits, Docker verification, native package script, standard security scan
- Produces: verified public-release candidate on `main`, with no remote push or GitHub tag

- [ ] **Step 1: Run fresh full verification**

```bash
npm run container:check
npm run verify:production-bundle
npm run test:release
node scripts/check-public-repo.mjs
npm run package:unsigned
```

Expected: all commands exit 0. The Docker target remains cache-only and does not export a runnable image.

- [ ] **Step 2: Inspect the packaged artifact**

Expand the archive into a fresh temporary directory and verify codesign, hardened runtime, arm64 executable, bundled TDLib checksum/dependencies, application version/minimum macOS, manifest values, and checksum. List archive paths and confirm there are no `.env`, `.db`, `.log`, session, job, source-map, fixture, or real screenshot artifacts.

- [ ] **Step 3: Run final repository security review**

Run `codex-security:security-scan` on the complete repository. Resolve validated critical/high findings and rerun affected TDD cycles plus full verification. Record lower-severity accepted preview risks in the threat model rather than hiding them.

- [ ] **Step 4: Review requirements and repository state**

Check every acceptance criterion in the specification against a file, test, or inspected artifact. Run:

```bash
git status --short --branch
git log --oneline --decorate -12
git diff fc57669..HEAD --stat
```

Expected: branch `main`, clean working tree, coherent task commits, no remote push, no release tag.

- [ ] **Step 5: Perform a focused final code review and resolve findings**

Review the complete diff from base `fc57669` against the specification, with separate passes for production/demo isolation, secret handling, workflow permissions, archive policy, user instructions, and deletion-scope regressions. Subagent delegation is unavailable for this task, so combine this review with the independent Codex Security scan rather than skipping the review gate. Fix Critical and Important findings, rerun the complete verification set, and commit corrections with a scoped message.

- [ ] **Step 6: Finish the development branch**

Use `superpowers:finishing-a-development-branch`. Because the work is already authorized in the newly initialized public repository, retain `main` locally and report the exact remote-creation/push/tag commands as the only manual publication steps.
