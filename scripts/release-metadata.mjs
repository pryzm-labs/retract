import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const EXPECTED_TDLIB_ARCHIVE_SHA256 = "aab5736f737319a13bcb871aa2b8a7a90a33e28ec9708fee53dbd387a24b98e4";
export const EXPECTED_TDLIB_COMPRESSED_SHA256 = "e93e2134e9fb57f7d019a8802f8518b9c0339806224af17df21fa965785afb95";

function requireMatch(value, pattern, label) {
  const match = value.match(pattern);
  if (!match) throw new Error(`invalid ${label}`);
  return match;
}

export function parseBuildStamp(text) {
  const lines = text.trim().split(/\r?\n/);
  if (lines.length !== 4) throw new Error("invalid TDLib build stamp");
  const provenance = requireMatch(
    lines[0],
    /^tdlib=([^\s]+) commit=([0-9a-f]{40}) arch=(arm64) macos=([^\s]+) linkage=(static)$/,
    "TDLib provenance"
  );
  const archiveDigest = requireMatch(
    lines[1],
    /^archive_sha256=([0-9a-f]{64}) file=libtdjson_retract\.a$/,
    "TDLib archive digest"
  );
  const compressedDigest = requireMatch(
    lines[2],
    /^compressed_sha256=([0-9a-f]{64}) file=libtdjson_static\.a\.gz$/,
    "TDLib compressed digest"
  );
  const openssl = requireMatch(
    lines[3],
    /^openssl=(3\.6\.3) provider=(sqlcipher-vendored-openssl)$/,
    "TDLib OpenSSL provider"
  );
  return {
    version: provenance[1],
    commit: provenance[2],
    architecture: provenance[3],
    linkage: provenance[5],
    archiveSha256: archiveDigest[1],
    compressedSha256: compressedDigest[1],
    opensslVersion: openssl[1],
    opensslProvider: openssl[2]
  };
}

export function assertVersionConsistency(versions) {
  const entries = Object.entries(versions);
  const distinct = new Set(entries.map(([, version]) => version));
  if (entries.length !== 5 || distinct.size !== 1 || !entries[0][1]) {
    throw new Error("Retract application versions must match");
  }
  return entries[0][1];
}

export function buildManifest(input) {
  if (input.product !== "Retract") throw new Error("invalid release product");
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(input.version)) {
    throw new Error("invalid release version");
  }
  if (!/^[0-9a-f]{7,64}$/.test(input.sourceCommit)) throw new Error("invalid source commit");
  if (input.target !== "aarch64-apple-darwin") throw new Error("invalid release target");
  if (input.tdlib.architecture !== "arm64") throw new Error("invalid TDLib architecture");
  if (input.tdlib.linkage !== "static"
      || input.tdlib.archiveSha256 !== EXPECTED_TDLIB_ARCHIVE_SHA256
      || input.tdlib.compressedSha256 !== EXPECTED_TDLIB_COMPRESSED_SHA256
      || input.tdlib.opensslVersion !== "3.6.3"
      || input.tdlib.opensslProvider !== "sqlcipher-vendored-openssl") {
    throw new Error("TDLib does not match Retract's reviewed static provenance");
  }
  return {
    product: input.product,
    version: input.version,
    sourceCommit: input.sourceCommit,
    target: input.target,
    minimumMacosVersion: input.minimumMacosVersion,
    signing: "ad-hoc",
    notarized: false,
    tdlib: input.tdlib
  };
}

function cargoPackageVersion(text, label) {
  const packageSection = requireMatch(text, /\[package\]([\s\S]*?)(?:\n\[|$)/, `${label} package section`)[1];
  return requireMatch(packageSection, /^version\s*=\s*"([^"]+)"\s*$/m, `${label} version`)[1];
}

export function readReleaseInput(projectRoot = resolve(fileURLToPath(new URL("..", import.meta.url)))) {
  const packageManifest = JSON.parse(readFileSync(resolve(projectRoot, "package.json"), "utf8"));
  const tauriManifest = JSON.parse(readFileSync(resolve(projectRoot, "src-tauri/tauri.conf.json"), "utf8"));
  const rustVersion = cargoPackageVersion(
    readFileSync(resolve(projectRoot, "src-tauri/Cargo.toml"), "utf8"),
    "Rust application"
  );
  const domainVersion = cargoPackageVersion(
    readFileSync(resolve(projectRoot, "crates/cleaner-domain/Cargo.toml"), "utf8"),
    "domain crate"
  );
  const neutralDomainVersion = cargoPackageVersion(
    readFileSync(resolve(projectRoot, "crates/retract-domain/Cargo.toml"), "utf8"),
    "neutral domain crate"
  );
  const version = assertVersionConsistency({
    packageVersion: packageManifest.version,
    tauriVersion: tauriManifest.version,
    rustVersion,
    domainVersion,
    neutralDomainVersion
  });
  const sourceCommit = process.env.RETRACT_SOURCE_COMMIT
    || execFileSync("git", ["rev-parse", "HEAD"], { cwd: projectRoot, encoding: "utf8" }).trim();
  return {
    product: tauriManifest.productName,
    version,
    sourceCommit,
    target: "aarch64-apple-darwin",
    minimumMacosVersion: tauriManifest.bundle.macOS.minimumSystemVersion,
    tdlib: parseBuildStamp(readFileSync(resolve(projectRoot, "vendor/tdlib-dist/build-stamp.txt"), "utf8"))
  };
}

function main() {
  const output = process.argv[2];
  if (!output) throw new Error("usage: node scripts/release-metadata.mjs OUTPUT_PATH");
  const destination = resolve(output);
  mkdirSync(dirname(destination), { recursive: true });
  writeFileSync(destination, `${JSON.stringify(buildManifest(readReleaseInput()), null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
