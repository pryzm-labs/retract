import assert from "node:assert/strict";
import test from "node:test";

import { assertVersionConsistency, buildManifest, parseBuildStamp } from "./release-metadata.mjs";

const tdlib = {
  version: "1.8.64",
  commit: "e0943d068ce90b5010f1aea946e6901e25b43bf6",
  architecture: "arm64",
  sha256: "a89780da629bce37eadba34448622141d89f37bb29a28dfe7293496d5b8ea044"
};

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
