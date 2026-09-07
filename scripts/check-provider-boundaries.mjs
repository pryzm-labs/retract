#!/usr/bin/env node

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { resolve, relative, sep } from "node:path";

const sourceRoot = resolve(process.argv[2] ?? "src-tauri/src");
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

if (failures.length > 0) {
  console.error("Telegram provider boundary check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exitCode = 1;
} else {
  console.log("Telegram provider boundaries are valid.");
}
