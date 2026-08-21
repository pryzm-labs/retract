import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

import { assertVersionConsistency, buildManifest, parseBuildStamp } from "./release-metadata.mjs";

const tdlib = {
  version: "1.8.64",
  commit: "e0943d068ce90b5010f1aea946e6901e25b43bf6",
  architecture: "arm64",
  sha256: "a89780da629bce37eadba34448622141d89f37bb29a28dfe7293496d5b8ea044"
};

const expectedBundleFiles = [
  "Contents/Info.plist",
  "Contents/MacOS/retract",
  "Contents/Resources/icon.icns",
  "Contents/Resources/lib/libtdjson.dylib",
  "Contents/Resources/licenses/TDLib-LICENSE_1_0.txt",
  "Contents/Resources/licenses/TDLib-build-stamp.txt",
  "Contents/_CodeSignature/CodeResources"
];

function createExpectedBundle(root) {
  for (const relativePath of expectedBundleFiles) {
    const absolutePath = join(root, relativePath);
    mkdirSync(resolve(absolutePath, ".."), { recursive: true });
    writeFileSync(absolutePath, relativePath);
  }
}

function verifyBundle(root) {
  return spawnSync("sh", [resolve("scripts/verify-app-contents.sh"), root], {
    encoding: "utf8"
  });
}

function verifyProductionBundle(root) {
  return spawnSync("sh", [resolve("scripts/verify-production-bundle-contents.sh"), root], {
    encoding: "utf8"
  });
}

function verifyCodesignDetails(path) {
  return spawnSync("sh", [resolve("scripts/verify-codesign-details.sh"), path], {
    encoding: "utf8"
  });
}

test("rejects inconsistent application versions", () => {
  assert.throws(() => assertVersionConsistency({
    packageVersion: "0.1.0",
    tauriVersion: "0.1.1",
    rustVersion: "0.1.0",
    domainVersion: "0.1.0"
  }), /versions must match/);
});

test("parses pinned TDLib provenance", () => {
  assert.deepEqual(parseBuildStamp(
    "tdlib=1.8.64 commit=e0943d068ce90b5010f1aea946e6901e25b43bf6 arch=arm64 macos=12.0\n" +
    "sha256=a89780da629bce37eadba34448622141d89f37bb29a28dfe7293496d5b8ea044 file=libtdjson.dylib\n"
  ), tdlib);
});

test("build manifest identifies an unsigned arm64 preview", () => {
  assert.deepEqual(buildManifest({
    product: "Retract",
    version: "0.1.0",
    sourceCommit: "0123456789abcdef",
    target: "aarch64-apple-darwin",
    minimumMacosVersion: "12.0",
    tdlib
  }), {
    product: "Retract",
    version: "0.1.0",
    sourceCommit: "0123456789abcdef",
    target: "aarch64-apple-darwin",
    minimumMacosVersion: "12.0",
    signing: "ad-hoc",
    notarized: false,
    tdlib
  });
});

test("release bundle validator accepts only the reviewed app contents", () => {
  const directory = mkdtempSync(join(tmpdir(), "retract-bundle-test-"));
  try {
    const valid = join(directory, "valid.app");
    createExpectedBundle(valid);
    assert.equal(verifyBundle(valid).status, 0);

    const unexpected = join(directory, "unexpected.app");
    createExpectedBundle(unexpected);
    writeFileSync(join(unexpected, "Contents/Resources/session.db"), "private state");
    const unexpectedResult = verifyBundle(unexpected);
    assert.notEqual(unexpectedResult.status, 0);
    assert.match(unexpectedResult.stderr, /unexpected app-bundle path/i);

    const linked = join(directory, "linked.app");
    createExpectedBundle(linked);
    symlinkSync("Info.plist", join(linked, "Contents/linked-plist"));
    const linkedResult = verifyBundle(linked);
    assert.notEqual(linkedResult.status, 0);
    assert.match(linkedResult.stderr, /symbolic links are not permitted/i);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("production bundle validator rejects fixture data without nonstandard tools", () => {
  const directory = mkdtempSync(join(tmpdir(), "retract-production-bundle-test-"));
  try {
    const clean = join(directory, "clean");
    mkdirSync(clean);
    writeFileSync(join(clean, "app.js"), "const product = 'Retract';");
    assert.equal(verifyProductionBundle(clean).status, 0);

    const contaminated = join(directory, "contaminated");
    mkdirSync(contaminated);
    writeFileSync(join(contaminated, "app.js"), "const command = 'reset_demo';");
    const contaminatedResult = verifyProductionBundle(contaminated);
    assert.notEqual(contaminatedResult.status, 0);
    assert.match(contaminatedResult.stderr, /forbidden fixture marker: reset_demo/i);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("codesign validator requires an ad-hoc signature with hardened runtime", () => {
  const directory = mkdtempSync(join(tmpdir(), "retract-codesign-test-"));
  try {
    const valid = join(directory, "valid.txt");
    writeFileSync(valid,
      "CodeDirectory v=20500 size=106260 flags=0x10002(adhoc,runtime) hashes=3314+3 location=embedded\n" +
      "Signature=adhoc\n"
    );
    assert.equal(verifyCodesignDetails(valid).status, 0);

    const signed = join(directory, "signed.txt");
    writeFileSync(signed,
      "CodeDirectory v=20500 size=106260 flags=0x10000(runtime) hashes=3314+3 location=embedded\n" +
      "Authority=Developer ID Application: Example\n"
    );
    const signedResult = verifyCodesignDetails(signed);
    assert.notEqual(signedResult.status, 0);
    assert.match(signedResult.stderr, /not ad-hoc signed/i);

    const unhardened = join(directory, "unhardened.txt");
    writeFileSync(unhardened,
      "CodeDirectory v=20400 size=106260 flags=0x2(adhoc) hashes=3314+3 location=embedded\n" +
      "Signature=adhoc\n"
    );
    const unhardenedResult = verifyCodesignDetails(unhardened);
    assert.notEqual(unhardenedResult.status, 0);
    assert.match(unhardenedResult.stderr, /hardened runtime/i);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
