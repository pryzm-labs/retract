#!/usr/bin/env node

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { resolve, relative, sep } from "node:path";

const sourceRoot = resolve(process.argv[2] ?? "src-tauri/src");
const archiveRoot = resolve(process.argv[3] ?? "crates/discord-archive/src");
const failures = [];

function display(path) {
  return relative(sourceRoot, path).split(sep).join("/");
}

function rustFiles(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return rustFiles(path);
    return entry.isFile() && entry.name.endsWith(".rs") ? [path] : [];
  });
}

function reject(path, expression, description) {
  const source = readFileSync(path, "utf8");
  if (expression.test(source)) failures.push(`${display(path)}: ${description}`);
}

for (const legacy of ["live_gateway.rs", "tdjson.rs", "model.rs", "service.rs", "gateway.rs", "providers/telegram/compat.rs", "providers/telegram/application.rs"]) {
  const path = resolve(sourceRoot, legacy);
  if (existsSync(path) && statSync(path).isFile()) {
    failures.push(`${legacy}: Telegram-native implementation must live under providers/telegram`);
  }
}

for (const required of [
  "providers/telegram/model.rs",
  "providers/telegram/native/live_gateway.rs",
  "providers/telegram/native/tdjson.rs",
  "providers/telegram/native/ports.rs",
]) {
  if (!existsSync(resolve(sourceRoot, required))) {
    failures.push(`${required}: required Telegram provider boundary file is missing`);
  }
}

const neutralFiles = [
  "provider_service.rs",
  "providers/ports.rs",
  "providers/registry.rs",
  "providers/lifecycle.rs",
  "providers/frozen_lifecycle.rs",
];
for (const name of neutralFiles) {
  const path = resolve(sourceRoot, name);
  if (!existsSync(path)) continue;
  reject(
    path,
    /(?:crate\s*::\s*(?:gateway|service)\b|\bTelegramGateway\b|\b(?:crate|super|self)\s*::[^;]*\btelegram\b|\bproviders\s*::[^;]*\btelegram\b|\btelegram\s*::)/s,
    "provider-neutral production code depends on the Telegram compatibility boundary",
  );
}

const query = resolve(sourceRoot, "providers/telegram/query.rs");
if (existsSync(query)) {
  reject(
    query,
    /\b(?:TelegramMutation|TelegramStateRepository|FoundationStore|SecureJobStore|GrantBook|CleanerService|TelegramCleanup|ScopedRepository)\b/,
    "production Telegram query owns mutation, cleanup state, or authorization",
  );
}

for (const module of ["recipe", "normalize", "diagnostics"]) {
  const path = resolve(sourceRoot, `providers/telegram/${module}.rs`);
  if (!existsSync(path)) continue;
  reject(
    path,
    /\b(?:TelegramCleanup|TelegramRead|TelegramMutation|TelegramStateRepository|FoundationStore|GrantBook)\b/,
    "pure Telegram codecs and diagnostics depend on runtime cleanup or native ports",
  );
}

for (const path of rustFiles(sourceRoot)) {
  const name = display(path);
  if (name.endsWith("_tests.rs") || name.endsWith("/tests.rs")) {
    continue;
  }
  reject(
    path,
    /\b(?:TelegramGateway|CleanerService|TelegramCompatibilityProvider|SessionGateway)\b|\bcrate\s*::\s*(?:service|gateway)\s*::/,
    "production code uses the temporary combined Telegram gateway or cleanup facade",
  );
}

// Source checks catch accidental coupling, not hostile obfuscation or arbitrary
// code execution. Production parser code has no filesystem or runtime services.
for (const path of rustFiles(archiveRoot)) {
  reject(path,
    /\b(?:reqwest|hyper|ureq|curl|surf|isahc|tokio|async_std|TcpStream|TcpListener|UdpSocket|tauri|keyring|security_framework|credentials?|secure_store|Keychain|telegram|Telegram\w*|rusqlite|sqlx|diesel|sqlite|SQL|webbrowser|opener|Command)\b|\b(?:std\s*::\s*)?(?:net|process)\s*::/,
    "archive parser isolation forbids network, UI, credentials, Telegram, SQL and process/browser APIs");
  reject(path,
    /\b(?:write|write_all|write_fmt|create|create_new|create_dir|create_dir_all|remove_file|remove_dir|remove_dir_all|rename|copy|hard_link|symlink|set_permissions)\s*(?:\(|\bas\b)|\b(?:write|create|create_new|create_dir|create_dir_all|remove_file|remove_dir|remove_dir_all|rename|copy|hard_link|symlink|set_permissions)\s*[,}]/,
    "archive parser isolation forbids filesystem write/create/remove APIs");
  reject(path, /\.\s*(?:append|truncate)\s*\(/,
    "archive parser isolation forbids writable file options");
}

const archiveManifest = resolve(archiveRoot, "../Cargo.toml");
if (existsSync(archiveManifest)) {
  const allowed = new Set(["serde", "serde_json", "sha2", "thiserror", "zip"]);
  let dependencySection = false;
  for (const original of readFileSync(archiveManifest, "utf8").split(/\r?\n/)) {
    const line = original.trim();
    if (line.startsWith("[")) {
      dependencySection = line === "[dependencies]";
      if (line.includes("dependencies") && !dependencySection) {
        failures.push("archive parser dependency must use the reviewed direct dependency table");
      }
    } else if (dependencySection && line && !line.startsWith("#")) {
      const dependency = line.match(/^([A-Za-z0-9_-]+)\s*=/)?.[1];
      if (!allowed.has(dependency) || /\b(?:package|path|git|workspace)\s*=/.test(line)) {
        failures.push("archive parser dependency is outside the reviewed allowlist");
      }
    }
  }
}

if (failures.length > 0) {
  console.error("Telegram provider boundary check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exitCode = 1;
} else {
  console.log("Telegram provider boundaries are valid.");
}
