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
function rustCode(source, preserveLiterals = false) {
  const masked = source.split("");
  for (let at = 0; at < source.length;) {
    let end = at;
    let literal = false;
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
        literal = true;
        const close = source.indexOf(`"${raw[1]}`, at + raw[0].length);
        end = close < 0 ? source.length : close + 1 + raw[1].length;
      } else if (source[at] === '"') {
        literal = true;
        end = at + 1;
        while (end < source.length && source[end] !== '"') end += source[end] === "\\" ? 2 : 1;
        end = Math.min(end + 1, source.length);
      } else if (character) { literal = true; end = at + character[0].length; }
    }
    if (end === at) { at++; continue; }
    if (literal && preserveLiterals) { at = end; continue; }
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

// Evaluate cfg with test=false and all other predicates unknown. In particular,
// any(test, feature=...) is production-capable and must never hide source.
function absentOutsideTests(attributes) {
  return [...attributes.matchAll(/#\s*\[\s*cfg\s*\(([\s\S]*?)\)\s*\]/g)].some((attribute) => {
    const tokens = attribute[1].match(/\w+|[(),=]/g) ?? [];
    let at = 0;
    function condition() {
      const name = tokens[at++];
      if (tokens[at] !== "(") {
        const plainTest = name === "test" && tokens[at] !== "=";
        while (at < tokens.length && ![",", ")"].includes(tokens[at])) at++;
        return plainTest ? false : null;
      }
      at++;
      const args = [];
      while (at < tokens.length && tokens[at] !== ")") {
        args.push(condition());
        if (tokens[at] === ",") at++;
        else if (tokens[at] !== ")") return null;
      }
      if (tokens[at++] !== ")") return null;
      if (name === "not" && args.length === 1) return args[0] === null ? null : !args[0];
      if (name === "all") return args.includes(false) ? false : args.includes(null) ? null : true;
      if (name === "any") return args.includes(true) ? true : args.includes(null) ? null : false;
      return null;
    }
    const result = condition();
    return at === tokens.length && result === false;
  });
}

const rustAttributes = String.raw`(?:#\s*\[[^\]]*\]\s*)*`;
const visibility = String.raw`(?:pub(?:\([^)]*\))?\s+)?`;
function moduleDirectory(name) {
  return name === "lib.rs" || name === "main.rs" || name === "mod.rs" || name.endsWith("/mod.rs")
    ? name.slice(0, name.lastIndexOf("/") + 1) : `${name.slice(0, -3)}/`;
}

function moduleDeclarations(name, code, source) {
  const inline = [];
  const declarations = [];
  const expression = new RegExp(`(${rustAttributes})${visibility}mod\\s+(\\w+)\\s*([;{])`, "g");
  let scanned = 0;
  let depth = 0;
  for (const match of code.matchAll(expression)) {
    for (; scanned < match.index; scanned++) {
      if (code[scanned] === "{") depth++;
      if (code[scanned] === "}") depth--;
    }
    const parents = inline.filter((parent) => parent.start < match.index && match.index < parent.end);
    // Declarations inside function/macro/use blocks cannot exclude sibling files.
    if (depth !== parents.length) continue;
    let path = `${moduleDirectory(name)}${parents.map((parent) => `${parent.name}/`).join("")}${match[2]}`;
    const testOnly = absentOutsideTests(match[1]) || parents.some((parent) => parent.testOnly);
    if (/#\s*\[\s*path\s*=/.test(match[1])) {
      const attributes = source.slice(match.index, match.index + match[1].length);
      const override = attributes.match(/#\s*\[\s*path\s*=\s*"([^"\\]*)"\s*\]/)?.[1];
      if (override === undefined) continue; // Unsupported syntax never grants an exemption.
      const parentDirectory = name.slice(0, name.lastIndexOf("/") + 1) + parents.map((parent) => `${parent.name}/`).join("");
      const target = display(resolve(sourceRoot, parentDirectory, override));
      if (target.startsWith("../") || !target.endsWith(".rs")) continue;
      path = target.replace(/(?:\/mod)?\.rs$/, "");
    }
    declarations.push({ path, testOnly, external: match[3] === ";" });
    if (match[3] === "{") {
      const start = match.index + match[0].length - 1;
      inline.push({ name: match[2], start, end: balancedEnd(code, start), testOnly });
    }
  }
  return declarations;
}

function withoutTestItems(code) {
  // Mask only a recognized item's own balanced extent. Unknown cfg-decorated
  // syntax stays visible instead of searching ahead into a later production item.
  const expression = new RegExp(`(${rustAttributes})${visibility}(?:(?:async|unsafe|const|extern)\\s+)*(mod|fn|use|type|struct|enum|impl|trait|const|static)\\b`, "g");
  for (const match of code.matchAll(expression)) {
    if (!absentOutsideTests(match[1])) continue;
    const semicolonItem = ["use", "type", "const", "static"].includes(match[2]);
    let end = match.index + match[0].length;
    for (; end < code.length; end++) {
      if (code[end] === "}") break;
      if (code[end] === ";") { end++; break; }
      if (["(", "[", "{"].includes(code[end])) {
        const brace = code[end] === "{";
        end = balancedEnd(code, end);
        if (brace && !semicolonItem) break;
        end--;
      }
    }
    code = code.slice(0, match.index) + " ".repeat(end - match.index) + code.slice(end);
  }
  return code;
}

function useBindings(code) {
  const bindings = [];
  for (const match of code.matchAll(/\buse\s+([^;]+);/g)) {
    const tokens = match[1].match(/\w+|::|[{},*]/g) ?? [];
    let at = 0;
    function tree(prefix) {
      const path = [...prefix];
      while (at < tokens.length && !["{", "}", ",", "as"].includes(tokens[at])) {
        const token = tokens[at++];
        if (token !== "::") path.push(token);
      }
      if (tokens[at] === "{") {
        at++;
        while (at < tokens.length && tokens[at] !== "}") {
          tree(path);
          if (tokens[at] === ",") at++;
        }
        at++;
      } else {
        let binding = path.at(-1) === "self" ? path.at(-2) : path.at(-1);
        if (tokens[at] === "as") { at++; binding = tokens[at++]; }
        bindings.push({ path, binding });
      }
    }
    tree([]);
  }
  return bindings;
}

function inheritedCode(name, productionCode) {
  const ancestors = [productionCode.get("lib.rs") ?? "", productionCode.get(name) ?? ""];
  const directories = name.split("/").slice(0, -1);
  for (let length = 1; length <= directories.length; length++) {
    const parent = directories.slice(0, length).join("/");
    ancestors.push(productionCode.get(`${parent}.rs`) ?? "", productionCode.get(`${parent}/mod.rs`) ?? "");
  }
  return ancestors.join("\n");
}

function discordAliases(code) {
  const imports = useBindings(code);
  const aliases = new Set();
  let changed = true;
  while (changed) {
    changed = false;
    for (const { path, binding } of imports) {
      if (binding && !aliases.has(binding) && path.some((part) => /discord/i.test(part) || aliases.has(part))) {
        aliases.add(binding);
        changed = true;
      }
    }
  }
  return aliases;
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
const nativeSource = new Map(rustFiles(sourceRoot).map((path) => [display(path), readFileSync(path, "utf8")]));
const nativeCode = new Map([...nativeSource].map(([name, source]) => [name, rustCode(source)]));
const modules = [...nativeCode].flatMap(([name, code]) => moduleDeclarations(name, code, rustCode(nativeSource.get(name), true)));
// One test branch cannot hide another production-capable declaration. Inline
// modules have descendants, but do not own a same-named sibling .rs file.
const testModules = modules.filter((module) => module.testOnly && !modules.some((other) => other.path === module.path && !other.testOnly));
const productionCode = new Map([...nativeCode]
  .filter(([name]) => !testModules.some((module) => (module.external && name === `${module.path}.rs`) || name.startsWith(`${module.path}/`)))
  .map(([name, code]) => [name, withoutTestItems(code)]));
for (const [name, code] of productionCode) {
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
  const aliases = discordAliases(inheritedCode(name, productionCode));
  if (scopes.some((scope) => /\b\w*discord\w*\b/i.test(scope) || aliases.has("*") || (scope.match(/\b\w+\b/g) ?? []).some((token) => aliases.has(token)))) {
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
