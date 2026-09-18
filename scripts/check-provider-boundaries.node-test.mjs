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

test("Discord backend and benchmark reject network, browser, credentials and Telegram coupling", () => {
  for (const name of ["import", "normalize", "locators", "benchmark"]) {
    for (const source of ["use reqwest::Client;", "use std::net::{TcpStream};", "use webbrowser::open;", "use crate::secure_store::load_archive_index_key;", "use security_framework::passwords;", "use crate::providers::telegram::model;"]) {
      const root = fixture();
      write(root, `providers/discord/${name}.rs`, source);
      const result = check(root);
      assert.equal(result.status, 1, `${name}: ${source}`);
      assert.match(result.stderr, /Discord backend isolation/);
    }
  }
});

test("Discord guard accepts content-free ownership documentation", () => {
  const root = fixture();
  write(root, "providers/discord/import.rs", "/// Construction performs no credential operations.\npub struct Owner;\n");
  assert.equal(check(root).status, 0);
});

test("Discord start and retry capabilities cannot escape their parent module", () => {
  for (const method of ["start", "retry", "launch"]) {
    for (const visibility of ["pub(crate)", "pub", "pub(in crate)", "pub(in crate::providers)"]) {
      const root = fixture();
      write(root, "providers/discord/import.rs", `impl DiscordImportOwner { ${visibility} async fn ${method}(&self, file: File) {} }`);
      const result = check(root);
      assert.equal(result.status, 1, `${visibility} ${method}`);
      assert.match(result.stderr, /Discord import capability visibility/);
    }
  }
});

test("Discord parent cannot publish coordinator wrappers or re-exports", () => {
  for (const source of [
    "pub(crate) async fn begin(owner: &import::DiscordImportOwner, file: File) { owner.start(file).await; }",
    "pub fn resume(owner: &import::DiscordImportOwner, file: File, outcome: Outcome) { owner.retry(file, outcome); }",
    "pub(crate) use import::DiscordImportOwner;",
    "pub use self::{import::{DiscordImportOwner as Owner}};",
    "pub(crate) use import::*;",
  ]) {
    const root = fixture();
    write(root, "providers/discord/mod.rs", source);
    const result = check(root);
    assert.equal(result.status, 1, source);
    assert.match(result.stderr, /Discord parent cannot expose import capabilities/);
  }
});

test("Discord modules outside the reviewed command adapter cannot host IPC", () => {
  for (const name of ["nested/tests.rs", "mod.rs", "import.rs"]) {
    for (const source of ["builder.invoke_handler(router);", "generate_handler![load];", "#[tauri::command] fn load() {}", "use tauri::command as exposed; #[exposed] fn load() {}"]) {
      const root = fixture();
      write(root, `providers/discord/${name}`, source);
      const result = check(root);
      assert.equal(result.status, 1, `${name}: ${source}`);
      assert.match(result.stderr, /Discord modules cannot host IPC/);
    }
  }
});

test("private capability methods coexist with crate-visible inert lifecycle methods", () => {
  const root = fixture();
  write(root, "providers/discord/import.rs", `
    impl DiscordImportOwner {
      pub(super) async fn start(&self, file: File) {}
      pub(self) async fn retry(&self, file: File, expected: Outcome) {}
      async fn launch(&self, file: File) {}
      pub(crate) fn new(archives: ArchiveOwner) {}
      pub(crate) fn reject_new_starts(&self) {}
      pub(crate) async fn shutdown(&self) {}
    }
  `);
  assert.equal(check(root).status, 0);
});

test("Discord capabilities remain unavailable outside the reviewed command adapter", () => {
  for (const name of ["commands.rs", "commands/archive.rs", "providers/registry.rs"]) {
    for (const source of ["use crate::providers::discord::import::DiscordImportOwner;", "use crate::providers::{discord::import};", "owner.discord_imports.start(file).await;"]) {
      const root = fixture();
      write(root, name, source);
      const result = check(root);
      assert.equal(result.status, 1, `${name}: ${source}`);
      assert.match(result.stderr, /reviewed provider command adapter/);
    }
  }
});

test("reviewed Discord command adapter can invoke the owned importer without direct I/O", () => {
  const root = fixture();
  write(root, "providers/discord/commands.rs", `
    use tauri::State;
    #[tauri::command]
    async fn start(runtime: State<'_, RuntimeState>) { runtime.discord_imports.start(file).await; }
  `);
  assert.equal(check(root).status, 0);
});

test("actual compatibility IPC modules reject Discord owner access through RuntimeState", () => {
  for (const name of ["compatibility/commands_v2.rs", "compatibility/model_v2.rs", "compatibility/mod.rs", "compatibility/archive/commands.rs"]) {
    const root = fixture();
    write(root, name, `
      use crate::RuntimeState;
      #[tauri::command]
      pub async fn import_archive(runtime: tauri::State<'_, Arc<RuntimeState>>) {
        runtime.discord_imports.start(file).await;
      }
    `);
    const result = check(root);
    assert.equal(result.status, 1, name);
    assert.match(result.stderr, /reviewed provider command adapter/);
  }
});

test("handler registration rejects direct Discord capability access", () => {
  for (const name of ["compatibility/commands_v2.rs", "lib.rs", "ipc/registration.rs"]) {
    for (const source of [
      "#[tauri::command] async fn import_archive(runtime: State<'_, Arc<RuntimeState>>) { runtime.discord_imports.start(file).await; }",
    ]) {
      const root = fixture();
      write(root, name, source);
      const result = check(root);
      assert.equal(result.status, 1, `${name}: ${source}`);
      assert.match(result.stderr, /reviewed provider command adapter/);
    }
  }
});

test("compatibility router may register reviewed Discord command functions", () => {
  const root = fixture();
  write(root, "compatibility/commands_v2.rs", "fn register(builder: Builder) { builder.invoke_handler(tauri::generate_handler![crate::providers::discord::commands::start_discord_import_v2]); }");
  assert.equal(check(root).status, 0);
});

test("lib entry point cannot delegate registration to a Discord command module", () => {
  const root = fixture();
  write(root, "lib.rs", "pub fn run() { let app = discord_commands::register(tauri::Builder::default()); }");
  const result = check(root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /reviewed provider command adapter/);
});

test("IPC guard permits inert owner lifecycle, documentation, literals and test-only modules", () => {
  const root = fixture();
  write(root, "lib.rs", `
    struct RuntimeState { discord_imports: DiscordImportOwner }
    impl RuntimeState { async fn shutdown(&self) { self.discord_imports.shutdown().await; } }
    pub fn run() { compatibility::commands_v2::register(tauri::Builder::default()); }
  `);
  write(root, "compatibility/mod.rs", "pub mod commands_v2; #[cfg(test)] mod fixtures; #[cfg(test)] mod tests;");
  write(root, "compatibility/fixtures.rs", "fn fixture(runtime: RuntimeState) { runtime.discord_imports.start(file); }");
  write(root, "compatibility/tests.rs", "fn discord_import_remains_unavailable() {}");
  write(root, "compatibility/commands_v2.rs", `
    /// Discord importer remains unavailable: runtime.discord_imports.start(file).
    /* Outer docs /* nested */ DiscordImportOwner is not used here. */
    #[tauri::command]
    fn snapshot() { let help = "Discord importer is unavailable"; let raw = r#"discord_imports"#; }
    fn register(builder: Builder) { builder.invoke_handler(tauri::generate_handler![snapshot]); }
  `);
  const result = check(root);
  assert.equal(result.status, 0, result.stderr);
});

test("IPC source checks do not infer cfg exclusions or module paths", () => {
  for (const declaration of [
    '#[cfg(test)] mod commands_v2;',
    'mod tests { #[cfg(test)] mod commands_v2; }',
    '#[cfg(any(test, feature = "ipc"))] mod commands_v2;',
    '#[path = "commands_v2.rs"] mod synthetic;',
  ]) {
    const root = fixture();
    write(root, "compatibility/mod.rs", declaration);
    write(root, "compatibility/commands_v2.rs", "use crate::providers::discord::import::*;");
    assert.equal(check(root).status, 1, declaration);
  }
  const root = fixture();
  write(root, "compatibility/commands_v2.rs", "#[cfg(test)] mod tests { use crate::providers::discord::import::*; }");
  assert.equal(check(root).status, 1);
});

test("direct coordinator aliases and wildcard imports are rejected in IPC files", () => {
  for (const source of [
    "use crate::providers::discord::import::DiscordImportOwner as Owner;",
    "use crate::{providers::{discord::{import::*}}};",
    "use crate::providers::discord::*;",
  ]) {
    const root = fixture();
    write(root, "compatibility/commands_v2.rs", source);
    assert.equal(check(root).status, 1, source);
  }
});

test("lexical IPC check does not taint unrelated shadowed aliases", () => {
  const root = fixture();
  write(root, "lib.rs", `
    use crate::providers::discord::import::DiscordImportOwner as Owner;
    mod ordinary { pub struct Owner; }
    mod ipc;
    pub fn run() { compatibility::commands_v2::register(Builder::default()); }
  `);
  write(root, "ipc.rs", "use crate::ordinary::Owner; #[tauri::command] fn snapshot(owner: Owner) {}");
  assert.equal(check(root).status, 0);
});

test("parser ZIP dependency cannot silently enable codecs or drift from the reviewed pin", () => {
  for (const dependency of ['zip = "8"', 'zip = { version = "=8.6.0", features = ["aes-crypto"] }', 'zip = { version = "=8.6.0", default-features = false, features = ["deflate-flate2-zlib-rs", "bzip2"] }']) {
    const root = fixture();
    write(root, "parser/src/lib.rs", "use std::io::Read;");
    write(root, "parser/Cargo.toml", `[dependencies]\n${dependency}\n`);
    const result = check(root, join(root, "parser/src"));
    assert.equal(result.status, 1, dependency);
    assert.match(result.stderr, /reviewed ZIP dependency/);
  }
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
