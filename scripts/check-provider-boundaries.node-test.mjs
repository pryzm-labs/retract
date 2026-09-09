import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const checker = new URL("./check-provider-boundaries.mjs", import.meta.url);

function write(root, name, source = "") {
  const path = join(root, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, source);
}

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "retract-provider-boundaries-"));
  for (const name of [
    "providers/telegram/model.rs",
    "providers/telegram/native/live_gateway.rs",
    "providers/telegram/native/tdjson.rs",
    "providers/telegram/native/ports.rs",
  ]) {
    write(root, name);
  }
  write(root, "provider_service.rs", "use crate::providers::ports::ApplicationQuery;\n");
  return root;
}

function check(root, archiveRoot = join(root, "archive-src")) {
  return spawnSync(process.execPath, [checker.pathname, root, archiveRoot], { encoding: "utf8" });
}

test("accepts a provider-owned native tree with neutral shared code", () => {
  const result = check(fixture());
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /boundaries are valid/);
});

test("archive parser accepts ordinary Read and Seek code", () => {
  const root = fixture();
  write(root, "archive-src/lib.rs", "use std::io::{Read, Seek}; pub fn inspect<R: Read + Seek>(reader: R) {}\n");
  const result = check(root);
  assert.equal(result.status, 0, result.stderr);
});

test("archive parser rejects network, UI, credentials, provider, SQL and browser coupling", () => {
  for (const source of [
    "use reqwest::Client;", "use hyper::Client;", "use ureq::Agent;", "use std::net::TcpStream;",
    "use tokio::net::TcpStream;", "use std::net::{UdpSocket};", "use tauri::State;",
    "use keyring::Entry;", "use security_framework::passwords;", "use crate::secure_store::Vault;",
    "use crate::credentials::Secret;", "use crate::Keychain;", "use crate::providers::telegram::Model;",
    "use crate::TelegramGateway;", "use rusqlite::Connection;", "use sqlx::Pool;",
    "use diesel::Connection;", "use webbrowser::open;", "use opener::open;",
    "std::process::Command::new(\"open\");", "use std::process::{Command as Spawn};",
  ]) {
    const root = fixture();
    write(root, "archive-src/lib.rs", source);
    const result = check(root);
    assert.equal(result.status, 1, source);
    assert.match(result.stderr, /archive parser isolation/);
  }
});

test("archive parser rejects filesystem mutations including grouped imports and aliases", () => {
  for (const source of [
    "std::fs::write(path, bytes);", "std::fs::File::create(path);", "std::fs::remove_file(path);",
    "use std::fs::{remove_dir_all};", "use std::fs::File as F; F::create(path);",
    "use std::fs::{write as save};", "options.write(true);", "options.append(true);",
    "options.create(true);", "options.truncate(true);", "file.write_all(bytes);",
    "std::fs::create_dir_all(path);", "std::fs::rename(a, b);", "std::fs::copy(a, b);",
  ]) {
    const root = fixture();
    write(root, "archive-src/nested/reader.rs", source);
    const result = check(root);
    assert.equal(result.status, 1, source);
    assert.match(result.stderr, /archive parser isolation/);
  }
});

test("archive parser manifest rejects forbidden and aliased dependencies", () => {
  for (const dependency of ["reqwest = \"1\"", "serde = { package = \"reqwest\", version = \"1\" }", "[target.'cfg(unix)'.dependencies]\nkeyring = \"1\"", "[dependencies.reqwest]\nversion = \"1\""]) {
    const root = fixture();
    write(root, "parser/src/lib.rs", "use std::io::Read;");
    write(root, "parser/Cargo.toml", `[package]\nname = "discord-archive"\n[dependencies]\n${dependency}\n`);
    const result = check(root, join(root, "parser/src"));
    assert.equal(result.status, 1, dependency);
    assert.match(result.stderr, /archive parser dependency/);
  }
});

test("rejects Telegram-native implementation outside the provider boundary", () => {
  const root = fixture();
  write(root, "live_gateway.rs", "pub struct LiveGateway;\n");
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /live_gateway\.rs: Telegram-native implementation/);
});

test("rejects Telegram compatibility dependencies from neutral production code", () => {
  const root = fixture();
  write(root, "providers/lifecycle.rs", "use crate::gateway::TelegramGateway;\n");
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /provider-neutral production code/);
});

test("rejects mutation and state ownership from production query", () => {
  const root = fixture();
  write(root, "providers/telegram/query.rs", "use super::native::ports::TelegramMutation;\n");
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /query owns mutation, cleanup state, or authorization/);
});

test("rejects the extracted cleanup owner from production query", () => {
  const root = fixture();
  write(root, "providers/telegram/query.rs", "use super::remediation::TelegramCleanup;\n");
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /query owns mutation, cleanup state, or authorization/);
});

test("rejects runtime construction dependencies in pure Telegram helpers", () => {
  for (const module of ["recipe", "normalize", "diagnostics"]) {
    const root = fixture();
    write(root, `providers/telegram/${module}.rs`, "use super::remediation::TelegramCleanup;\n");
    const result = check(root);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /pure Telegram codecs and diagnostics/);
  }
});

test("rejects the combined gateway through the Telegram relative re-export", () => {
  const root = fixture();
  write(
    root,
    "providers/telegram/query.rs",
    "use super::TelegramGateway;\nuse std::sync::Arc;\ntype QueryGateway = Arc<dyn TelegramGateway>;\n",
  );
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /temporary combined Telegram gateway/);
});

test("rejects grouped Telegram imports from neutral production code", () => {
  const root = fixture();
  write(
    root,
    "providers/lifecycle.rs",
    "use crate::providers::{telegram::model::SearchRequest};\n",
  );
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /provider-neutral production code/);
});

test("rejects the combined gateway from native files without a file exception", () => {
  const root = fixture();
  write(
    root,
    "providers/telegram/native/live_gateway.rs",
    "use crate::gateway::TelegramGateway;\n",
  );
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /temporary combined Telegram gateway/);
});

test("rejects every removed transitional file and production facade dependency", () => {
  for (const name of ["service.rs", "gateway.rs", "providers/telegram/compat.rs", "providers/telegram/application.rs"]) {
    const root = fixture();
    write(root, name);
    assert.equal(check(root).status, 1, name);
  }
  for (const symbol of ["CleanerService", "TelegramCompatibilityProvider", "SessionGateway"]) {
    const root = fixture();
    write(root, "providers/telegram/connection.rs", `use super::${symbol};\n`);
    const result = check(root);
    assert.equal(result.status, 1, symbol);
    assert.match(result.stderr, /cleanup facade/);
  }
});
