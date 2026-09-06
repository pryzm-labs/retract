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

function check(root) {
  return spawnSync(process.execPath, [checker.pathname, root], { encoding: "utf8" });
}

test("accepts a provider-owned native tree with neutral shared code", () => {
  const result = check(fixture());
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /boundaries are valid/);
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

test("allows only the named transitional compatibility files", () => {
  const valid = fixture();
  write(valid, "service.rs", "use crate::gateway::TelegramGateway;\n");
  assert.equal(check(valid).status, 0);

  const invalid = fixture();
  write(invalid, "providers/telegram/connection.rs", "use crate::gateway::TelegramGateway;\n");
  const result = check(invalid);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /temporary combined Telegram gateway/);
});
