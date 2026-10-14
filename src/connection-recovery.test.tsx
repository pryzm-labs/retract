import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import App from "./App";
import bootstrap from "./test/fixtures/connection-bootstrap.json";
import lifecycle from "./test/fixtures/provider-lifecycle.json";
import { api } from "./api.desktop";
import { lifecycleContext, wireMessage, wireSnapshot } from "./test/lifecycle-wire";

function installRecoveryTransport(mode: "replacement_fails" | "settings_read_fails" | "failed_startup") {
  let state: "a" | "failed" | "b" = mode === "failed_startup" ? "failed" : "a";
  let saves = 0;
  let settingsReadsAfterSave = 0;
  const transport = vi.mocked(invoke);
  transport.mockImplementation(async (command, args) => {
    const request = (args as { request: { context: unknown } }).request;
    const context = state === "a" ? lifecycle.context : state === "b" ? lifecycle.syntheticContext : null;
    const envelope = (payload: unknown) => ({ contractVersion: 2, context, payload });
    // Mirrors the Rust runtime: discovery alone may omit a ready context;
    // settings/auth/mutations remain bound even when the old service was retired.
    if (JSON.stringify(request.context) !== JSON.stringify(context) && !(command === "get_bootstrap_snapshot_v2" && request.context === null)) {
      throw { code: context === null ? "identity_unavailable" : "stale_context", retryAt: null };
    }
    if (command === "get_bootstrap_snapshot_v2" || command === "get_snapshot_v2") {
      return context ? wireSnapshot(context) : structuredClone(bootstrap.failed);
    }
    if (command === "get_connection_settings_v2") {
      if (saves && mode === "settings_read_fails" && settingsReadsAfterSave++ === 0) throw { code: "state_persistence_failed", retryAt: null };
      return envelope(lifecycle.connectionSettings);
    }
    if (command === "save_connection_settings_v2") {
      saves += 1;
      if (saves === 1 && mode === "replacement_fails") {
        state = "failed";
        throw structuredClone(bootstrap.failed.payload.identity.diagnostic);
      }
      state = "b";
      return wireSnapshot(lifecycle.syntheticContext);
    }
    if (command === "retry_identity_v2") { state = "b"; return wireSnapshot(lifecycle.syntheticContext); }
    if (command === "search_messages_v2") return envelope({ items: (state === "a" ? lifecycle.messages.slice(0, 2) : [lifecycle.messages[2]]).map(wireMessage), nextCursor: null });
    if (command === "get_jobs_v2" || command === "get_intents_v2") return envelope([]);
    throw new Error(`Unexpected command: ${command}`);
  });
  return transport;
}

// Rust's registered bootstrap serializer asserts these exact shared fixtures.
// Keep App, createApi and every decoder real; replace only native IPC.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@retract/api", async () => import("./api.desktop"));

describe("connection bootstrap and replacement recovery", () => {
  beforeEach(() => { vi.mocked(invoke).mockReset(); });

  it.each(["bootstrap", "settings", "contextless settings"])("rediscovers startup when the session advances during %s", async phase => {
    const settled = { ...lifecycle.context, sessionGeneration: "dddddddd-dddd-4ddd-8ddd-dddddddddddd" };
    let firstBootstrap = true;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      const request = (args as { request: { context: unknown } }).request;
      if (command === "get_bootstrap_snapshot_v2") {
        if (firstBootstrap) {
          firstBootstrap = false;
          if (phase === "bootstrap") throw { code: "stale_context", retryAt: null };
          // The unverified/contextless service can also become ready here.
          return phase === "contextless settings" ? structuredClone(bootstrap.setup) : wireSnapshot();
        }
        return wireSnapshot(settled);
      }
      // Real native reads reject the superseded session; do not relax this guard.
      if (JSON.stringify(request.context) !== JSON.stringify(settled)) throw { code: "stale_context", retryAt: null };
      const envelope = (payload: unknown) => ({ contractVersion: 2, context: settled, payload });
      if (command === "get_connection_settings_v2") return envelope(lifecycle.connectionSettings);
      if (command === "get_snapshot_v2") return wireSnapshot(settled);
      if (command === "search_messages_v2") return envelope({ items: lifecycle.messages.slice(0, 2).map(wireMessage), nextCursor: null });
      if (command === "get_jobs_v2" || command === "get_intents_v2") return envelope([]);
      throw new Error(`Startup must not issue a mutation: ${command}`);
    });
    render(<App />);
    expect(await screen.findByText("First lossless target")).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Workspace not ready" })).not.toBeInTheDocument();
    const catalogAndSearchReads = vi.mocked(invoke).mock.calls.filter(([command]) => command === "get_snapshot_v2" || command === "search_messages_v2");
    expect(catalogAndSearchReads.length).toBeGreaterThan(0);
    for (const [, args] of catalogAndSearchReads) expect(args).toMatchObject({ request: { context: settled } });
  });

  it("bounds repeated stale startup discovery and retains manual recovery", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "stale_context", retryAt: null });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Workspace not ready" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "get_bootstrap_snapshot_v2", "get_bootstrap_snapshot_v2", "get_bootstrap_snapshot_v2"
    ]);
  });

  it("does not rediscover or read settings after startup unmount", async () => {
    let rejectRead!: (reason: unknown) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((_resolve, reject) => { rejectRead = reject; }));
    const view = render(<App />);
    view.unmount();
    rejectRead({ code: "stale_context", retryAt: null });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual(["get_bootstrap_snapshot_v2"]);
  });

  it("ignores superseded StrictMode startup errors after the current workspace opens", async () => {
    const transport = installRecoveryTransport("settings_read_fails");
    const original = transport.getMockImplementation()!;
    let rejectOldRead!: (reason: unknown) => void;
    let first = true;
    transport.mockImplementation((command, args) => {
      if (first) {
        first = false;
        return new Promise((_resolve, reject) => { rejectOldRead = reject; });
      }
      return original(command, args);
    });
    render(<StrictMode><App /></StrictMode>);
    expect(await screen.findByText("First lossless target")).toBeVisible();
    rejectOldRead({ code: "stale_context", retryAt: null });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(screen.queryByRole("heading", { name: "Workspace not ready" })).not.toBeInTheDocument();
    expect(transport.mock.calls.filter(([command]) => command === "get_bootstrap_snapshot_v2")).toHaveLength(2);
  });

  it.each(["profile_in_use", "authentication_required", "state_persistence_failed", "scope_mismatch", "unsupported_schema"].flatMap(code =>
    ["bootstrap", "settings"].map(phase => ({ code, phase }))
  ))("does not automatically retry $phase error $code", async ({ code, phase }) => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (phase === "settings" && command === "get_bootstrap_snapshot_v2") return wireSnapshot();
      throw { code, retryAt: null };
    });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Workspace not ready" })).toBeVisible();
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual(phase === "bootstrap"
      ? ["get_bootstrap_snapshot_v2"]
      : ["get_bootstrap_snapshot_v2", "get_connection_settings_v2"]);
  });

  it("opens first-run onboarding from the actual serialized setup bootstrap", async () => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "get_bootstrap_snapshot_v2") return structuredClone(bootstrap.setup);
      if (command === "get_connection_settings_v2") return {
        contractVersion: 2, context: null,
        payload: { ...lifecycle.connectionSettings, setupComplete: false, apiId: null, apiHashConfigured: false }
      };
      throw new Error(`Unexpected command: ${command}`);
    });
    render(<App />);
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Choose what to clean up" })).toBeVisible()
    );
    fireEvent.click(screen.getByRole("button", { name: /Telegram/ }));
    expect(await screen.findByRole("heading", { name: "Connect Telegram" })).toBeVisible();
    expect(screen.queryByText("Workspace not ready")).not.toBeInTheDocument();
  });

  it("discovers the retired context after a failed replacement and permits an explicit corrected save", async () => {
    const transport = installRecoveryTransport("replacement_fails");
    render(<App />);
    fireEvent.click(await screen.findByText("First lossless target"));
    fireEvent.click(screen.getByRole("button", { name: /connection settings/i }));
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(transport.mock.calls.filter(([command]) => command === "get_bootstrap_snapshot_v2")).toHaveLength(2));
    expect(transport.mock.calls.filter(([command]) => command === "get_bootstrap_snapshot_v2").at(-1)?.[1]).toMatchObject({ request: { context: null } });
    expect(screen.queryByText("First lossless target")).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    expect(await screen.findByText("Current synthetic target")).toBeVisible();
    expect(transport.mock.calls.filter(([command]) => command === "save_connection_settings_v2").map(([, args]) => (args as { request: { context: unknown } }).request.context)).toEqual([lifecycle.context, null]);
  });

  it.each([false, true])("publishes the committed replacement when settings read fails (discovery fails: %s)", async discoveryFails => {
    const transport = installRecoveryTransport("settings_read_fails");
    const original = transport.getMockImplementation()!;
    let saved = false;
    transport.mockImplementation(async (command, args) => {
      if (saved && discoveryFails && command === "get_bootstrap_snapshot_v2") throw { code: "transient", retryAt: null };
      const result = await original(command, args);
      if (command === "save_connection_settings_v2") saved = true;
      return result;
    });
    render(<App />);
    fireEvent.click(await screen.findByText("First lossless target"));
    fireEvent.click(screen.getByRole("button", { name: /connection settings/i }));
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    expect(await screen.findByText("Current synthetic target")).toBeInTheDocument();
    expect(screen.queryByText("First lossless target")).not.toBeInTheDocument();
    expect(transport.mock.calls.filter(([command]) => command === "save_connection_settings_v2")).toHaveLength(1);
  });

  it("preserves structured safe error codes across the real desktop adapter", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "profile_in_use", retryAt: null });
    await expect(api.connectionSettings(lifecycleContext)).rejects.toMatchObject({ code: "profile_in_use", message: bootstrap.failed.payload.identity.diagnostic.message, retryAt: null });
  });

  it("shows a failed contextless connection diagnostic and recovers through explicit retry", async () => {
    const transport = installRecoveryTransport("failed_startup");
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent(bootstrap.failed.payload.identity.diagnostic.message);
    expect(screen.queryByText("Waiting for the local engine…")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry verification" }));
    expect(await screen.findByText("Current synthetic target")).toBeVisible();
    expect(transport.mock.calls.filter(([command]) => command === "retry_identity_v2")).toHaveLength(1);
  });
});
