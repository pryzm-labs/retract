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

// A lexical architecture guard, not a Rust compiler: ignore prose/literals so
// IPC checks concern identifiers and access paths, not documentation strings.
function rustCode(source) {
  const masked = source.split("");
  for (let at = 0; at < source.length;) {
    let end = at;
    if (source.startsWith("//", at)) {
      end = source.indexOf("\n", at);
      if (end < 0) end = source.length;
    } else if (source.startsWith("/*", at)) {
      end = at + 2;
      let depth = 1;
      while (end < source.length && depth) {
        if (source.startsWith("/*", end)) { depth++; end += 2; }
        else if (source.startsWith("*/", end)) { depth--; end += 2; }
        else end++;
      }
    } else {
      const raw = source.slice(at).match(/^(?:b|c)?r(#{0,255})"/);
      const character = source.slice(at).match(/^'(?:\\.|[^'\\\n])'/u);
      if (raw) {
        const close = source.indexOf(`"${raw[1]}`, at + raw[0].length);
        end = close < 0 ? source.length : close + 1 + raw[1].length;
      } else if (source[at] === '"') {
        end = at + 1;
        while (end < source.length && source[end] !== '"') end += source[end] === "\\" ? 2 : 1;
        end = Math.min(end + 1, source.length);
      } else if (character) end = at + character[0].length;
    }
    if (end === at) { at++; continue; }
    for (; at < end; at++) if (masked[at] !== "\n") masked[at] = " ";
  }
  return masked.join("");
}

function balancedEnd(code, start) {
  const closing = { "(": ")", "[": "]", "{": "}" };
  const stack = [closing[code[start]]];
  for (let at = start + 1; at < code.length; at++) {
    if (closing[code[at]]) stack.push(closing[code[at]]);
    else if (code[at] === stack.at(-1)) {
      stack.pop();
      if (!stack.length) return at + 1;
    }
  }
  return code.length;
}

function withoutTestItems(code) {
  const expression = /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]/g;
  for (const match of code.matchAll(expression)) {
    const item = /[;{]/g;
    item.lastIndex = match.index + match[0].length;
    const start = item.exec(code);
    if (!start) continue;
    const end = start[0] === ";" ? start.index + 1 : balancedEnd(code, start.index);
    code = code.slice(0, match.index) + " ".repeat(end - match.index) + code.slice(end);
  }
  return code;
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

for (const path of rustFiles(resolve(sourceRoot, "providers/discord"))) {
  if (path.endsWith("_tests.rs") || path.endsWith("/tests.rs")) continue;
  reject(path,
    /\b(?:reqwest|hyper|ureq|curl|surf|isahc|TcpStream|TcpListener|UdpSocket|tauri|keyring|security_framework|credentials|secure_store|Keychain|telegram|Telegram\w*|webbrowser|opener|Command)\b|\b(?:std\s*::\s*)?(?:net|process)\s*::/,
    "Discord backend isolation forbids network, UI, credential, Telegram and process/browser APIs");
}
const nativeCode = new Map(rustFiles(sourceRoot).map((path) => [display(path), rustCode(readFileSync(path, "utf8"))]));
const testModules = [];
for (const [name, code] of nativeCode) {
  for (const match of code.matchAll(/#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;/g)) {
    const parent = name === "lib.rs" || name.endsWith("/mod.rs") ? name.slice(0, name.lastIndexOf("/") + 1) : `${name.slice(0, -3)}/`;
    testModules.push(`${parent}${match[1]}`);
  }
}
for (const [name, raw] of nativeCode) {
  if (name.endsWith("_tests.rs") || name.endsWith("/tests.rs") || testModules.some((module) => name === `${module}.rs` || name.startsWith(`${module}/`))) continue;
  const code = withoutTestItems(raw);
  const scopes = [];
  if (name === "commands.rs" || name.startsWith("commands/") || name.startsWith("compatibility/") || name === "providers/registry.rs") scopes.push(code);
  // Catch commands and direct handlers even if moved outside compatibility.
  for (const match of code.matchAll(/#\s*\[\s*(?:tauri\s*::\s*)?command\b[^\]]*\]|\bgenerate_handler\s*!\s*[[({]|\.\s*invoke_handler\s*\(/g)) {
    const open = match[0].startsWith("#") ? code.indexOf("{", match.index + match[0].length) : match.index + match[0].length - 1;
    if (open >= 0) scopes.push(code.slice(match.index, balancedEnd(code, open)));
  }
  // lib.rs owns an inert importer lifecycle as well as the delegated router.
  // Inspect the entry point, not its unrelated owner construction/shutdown.
  if (name === "lib.rs") {
    for (const match of code.matchAll(/\bfn\s+run\s*\(/g)) {
      const open = code.indexOf("{", match.index + match[0].length);
      if (open >= 0) scopes.push(code.slice(match.index, balancedEnd(code, open)));
    }
  }
  if (scopes.some((scope) => /\b\w*discord\w*\b/i.test(scope))) {
    failures.push(`${name}: Discord importer is backend-only and cannot enter commands or provider registration`);
  }
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
      if (dependency === "zip" && !/^zip\s*=\s*\{\s*version\s*=\s*"=8\.6\.0",\s*default-features\s*=\s*false,\s*features\s*=\s*\["deflate-flate2-zlib-rs"\]\s*\}\s*(?:#.*)?$/.test(line)) {
        failures.push("archive parser must preserve the reviewed ZIP dependency pin and codec features");
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
