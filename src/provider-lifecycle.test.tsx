import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { api } from "./api.desktop";
import { messageKey } from "./components/ResultsList";
import fixture from "./test/fixtures/provider-lifecycle.json";
import type { MessageSnapshot } from "./types";
import { decodeContext, decodeScope, decodeContent, decodeRef } from "./providers/contract";
import { contentView } from "./providers/telegram";
import { wireMessage, wireJob, wireSnapshot, wirePlan, lifecycleContext, lifecycleMessages, lifecycleChats } from "./test/lifecycle-wire";

// Exercise App -> real desktop adapter. Only the native IPC transport is fake.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@retract/api", async () => import("./api.desktop"));

type FixtureMessage = typeof fixture.messages[number];
type FixtureContext = typeof fixture.context;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((finish) => { resolve = finish; });
  return { promise, resolve };
}

function installTransport(options: {
  messages?: FixtureMessage[];
  context?: FixtureContext;
  jobs?: unknown[];
  poll?: () => Promise<unknown[]>;
  refresh?: () => Promise<unknown[]>;
  search?: (query: string) => Promise<FixtureMessage[]>;
  switchContext?: FixtureContext;
  switchMessages?: FixtureMessage[];
} = {}) {
  let currentContext = options.context ?? fixture.context;
  let messages = options.messages ?? fixture.messages.slice(0, 2);
  const transport = vi.mocked(invoke);
  transport.mockImplementation(async (command, args) => {
    const captured = structuredClone(currentContext);
    const request = (args as { request: { payload: Record<string, unknown> } }).request;
    const envelope = (payload: unknown) => ({ contractVersion: 2, context: captured, payload });
    switch (command) {
      case "get_bootstrap_snapshot_v2":
      case "get_snapshot_v2": {
        const next = wireSnapshot(captured, options.jobs);
        next.payload.chats = next.payload.chats.map(chat => ({ ...chat, scope: captured.scope }));
        return next;
      }
      case "get_connection_settings_v2": return envelope(fixture.connectionSettings);
      case "search_messages_v2": {
        const filters = request.payload.filters as { payload: { contentKinds: string[] } };
        const visible = filters.payload.contentKinds.length ? [] : options.search && request.payload.query === "late" ? await options.search("late") : messages;
        return envelope({ items: visible.map(wireMessage), nextCursor: null });
      }
      case "prepare_selection_v2": return envelope(wirePlan(captured, request.payload.messageRefs as unknown[]));
      case "get_intents_v2": return envelope([]);
      case "get_jobs_v2": return envelope((options.poll ? await options.poll() : []).map(j => wireJob(j as typeof fixture.job)));
      case "refresh_chats_v2": return envelope(options.refresh ? await options.refresh() : []);
      case "save_connection_settings_v2":
        currentContext = options.switchContext ?? fixture.syntheticContext;
        messages = options.switchMessages ?? [fixture.messages[2]];
        return { ...wireSnapshot(currentContext), payload: { ...wireSnapshot(currentContext).payload, chats: wireSnapshot(currentContext).payload.chats.map(chat => ({ ...chat, scope: currentContext.scope })) } };
      default: throw new Error(`Unexpected native boundary command: ${command}`);
    }
  });
  return transport;
}

async function switchToSyntheticAccount() {
  fireEvent.click(screen.getByRole("button", { name: /connection settings/i }));
  fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
  await screen.findByText("Connection settings applied.");
  fireEvent.click(await screen.findByText("Current synthetic target"));
  expect(screen.getByText("Current synthetic target").closest("article")).toHaveClass("is-selected");
}

describe("provider foundation lifecycle migration gate", () => {
  beforeEach(() => { vi.mocked(invoke).mockReset(); });
  afterEach(() => { vi.useRealTimers(); });

  it("keeps identical native targets in different accounts distinct", () => {
    const first = lifecycleMessages[0];
    const otherAccount = { ...first, scope: decodeScope(fixture.otherScope) };
    expect(messageKey(first))
      .not.toBe(messageKey(otherAccount));
  });

  it.each([
    ["provider", { ...fixture.context.scope, provider: "synthetic" }],
    ["source", { ...fixture.context.scope, sourceId: "22222222-2222-4222-8222-222222222222" }]
  ])("includes %s in selection identity", (_field, scope) => {
    const first = lifecycleMessages[0];
    expect(messageKey(first))
      .not.toBe(messageKey({ ...first, scope: decodeScope(scope) }));
  });

  it("keeps complete same-native records from different providers distinct", () => {
    expect(messageKey(lifecycleMessages[0]))
      .not.toBe(messageKey(contentView(decodeContent(wireMessage(fixture.sameNativeOtherProviderMessage), decodeScope(fixture.sameNativeOtherProviderMessage.scope)))));
  });

  it("preserves both large string targets through actual selection and preparation", async () => {
    const transport = installTransport();
    render(<App />);
    fireEvent.click(await screen.findByText("First lossless target"));
    expect(screen.getByText("Second lossless target").closest("article")).toHaveClass("is-selected");
    fireEvent.click(screen.getByRole("button", { name: /Review deletion/ }));
    await screen.findByText("Delete 2 messages for everyone?");
    const dispatch = transport.mock.calls.find(([command]) => command.startsWith("prepare_selection"));
    expect(dispatch, "selection must reach the real desktop transport").toBeDefined();
    const request = (dispatch![1] as Record<string, unknown>)?.request as {
      messageRefs?: Array<{ messageId: unknown }>;
      payload?: { messageRefs: Array<{ resource: { locatorPayload: { messageId: unknown } } }> };
    };
    // Inspect the actual transport in either migration phase, never reconstruct IDs.
    const targetIds = request.payload
      ? request.payload.messageRefs.map((ref) => ref.resource.locatorPayload.messageId)
      : request.messageRefs?.map((ref) => ref.messageId);
    expect(targetIds).toEqual(["9007199254740992", "9007199254740993"]);
    expect(dispatch).toEqual(["prepare_selection_v2", { request: {
      contractVersion: 2,
      context: fixture.context,
      payload: { messageRefs: [fixture.messages[0].ref, fixture.messages[1].ref] }
    } }]);
  });

  it("keeps a nonnumeric synthetic target scoped through desktop IPC", async () => {
    const transport = installTransport({ context: fixture.syntheticContext, messages: [fixture.messages[2]] });
    render(<App />);
    fireEvent.click(await screen.findByText("Current synthetic target"));
    fireEvent.click(screen.getByRole("button", { name: /Review deletion/ }));
    await waitFor(() => expect(transport.mock.calls.some(([command]) => command.startsWith("prepare_selection"))).toBe(true));
    const dispatch = transport.mock.calls.find(([command]) => command.startsWith("prepare_selection"));
    expect(dispatch).toEqual(["prepare_selection_v2", { request: {
      contractVersion: 2,
      context: fixture.syntheticContext,
      payload: { messageRefs: [fixture.messages[2].ref] }
    } }]);
    expect(JSON.stringify(dispatch)).toContain("message:part/0007");
  });

  it("dispatches targeted refresh with context and scoped conversation refs", async () => {
    const transport = installTransport();
    await api.refreshChats([decodeRef(fixture.dirtyRefs[0])], lifecycleContext);
    expect(transport).toHaveBeenLastCalledWith("refresh_chats_v2", { request: {
      contractVersion: 2,
      context: fixture.context,
      payload: { conversations: [fixture.dirtyRefs[0]] }
    } });
  });

  it("does not expand an album into another conversation with the same grouping ID", async () => {
    installTransport({ messages: [fixture.messages[0], fixture.messages[1], fixture.messages[3]] });
    render(<App />);
    fireEvent.click(await screen.findByText("First lossless target"));
    expect(screen.getByText("Second lossless target").closest("article")).toHaveClass("is-selected");
    expect(screen.getByText("Other conversation album target").closest("article")).not.toHaveClass("is-selected");
  });

  it("retains hidden opaque selections when the media filter hides them", async () => {
    installTransport({ context: fixture.syntheticContext, messages: [fixture.messages[2]] });
    render(<App />);
    fireEvent.click(await screen.findByText("Current synthetic target"));
    fireEvent.click(screen.getByRole("button", { name: "Media" }));
    expect(await screen.findByText("1 selection outside this result view")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Review deletion/ })).toBeEnabled();
  });

  it("preserves the current selection when an old-context targeted refresh resolves", async () => {
    const refresh = deferred<unknown[]>();
    const transport = installTransport({
      jobs: [fixture.job],
      poll: async () => [{ ...fixture.job, status: "completed" }],
      refresh: () => refresh.promise
    });
    render(<App />);
    await screen.findByText("First lossless target");
    await waitFor(() => expect(transport.mock.calls.some(([command]) => command === "refresh_chats_v2")).toBe(true), { timeout: 2000 });
    await switchToSyntheticAccount();
    await act(async () => { refresh.resolve([]); });
    expect(screen.getByText("Current synthetic target").closest("article")).toHaveClass("is-selected");
    expect(within(screen.getByRole("complementary", { name: "Chat navigation" }))
      .getByRole("button", { name: /Synthetic nonnumeric conversation/ })).toBeInTheDocument();
  });

  it("ignores old-context job impacts instead of clearing the current account selection", async () => {
    const poll = deferred<unknown[]>();
    const transport = installTransport({ jobs: [fixture.job], poll: () => poll.promise });
    render(<App />);
    await screen.findByText("First lossless target");
    await waitFor(() => expect(transport.mock.calls.some(([command]) => command === "get_jobs_v2")).toBe(true), { timeout: 2000 });
    await switchToSyntheticAccount();
    await act(async () => { poll.resolve([{ ...fixture.job, status: "completed" }]); });
    expect(screen.getByText("Current synthetic target").closest("article"),
      "a completed old-account job must not reconcile the new account's matching native chat ID")
      .toHaveClass("is-selected");
    expect(transport.mock.calls.filter(([command]) => command.startsWith("refresh_chats"))).toEqual([]);
  });
});

const otherSource = { ...fixture.context, scope: { ...fixture.context.scope, sourceId: "22222222-2222-4222-8222-222222222222" } };
const sourceMessage = { ...fixture.messages[0], preview: "Current source target", scope: otherSource.scope,
  ref: { ...fixture.messages[0].ref, scope: otherSource.scope }, conversation: { ...fixture.messages[0].conversation, scope: otherSource.scope } };
describe("late v2 responses across account and source changes", () => {
  beforeEach(() => { vi.mocked(invoke).mockReset(); });
  it.each([
    ["account", fixture.syntheticContext, fixture.messages[2]],
    ["source", otherSource, sourceMessage]
  ] as const)("discards a stale search and clears review when the %s changes", async (_field, nextContext, message) => {
    const late = deferred<FixtureMessage[]>();
    const transport = installTransport({ search: () => late.promise, switchContext: nextContext, switchMessages: [message] });
    render(<App />);
    fireEvent.click(await screen.findByText("First lossless target"));
    fireEvent.click(screen.getByRole("button", { name: /Review deletion/ }));
    await screen.findByText("Delete 2 messages for everyone?");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.change(screen.getByPlaceholderText("Names, phrases, captions, or sensitive details"), { target: { value: "late" } });
    await waitFor(() => expect(transport.mock.calls.filter(([c]) => c === "search_messages_v2").length).toBeGreaterThan(1));
    fireEvent.click(screen.getByRole("button", { name: /connection settings/i }));
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    await screen.findByText("Connection settings applied.");
    fireEvent.change(screen.getByPlaceholderText("Names, phrases, captions, or sensitive details"), { target: { value: "" } });
    fireEvent.click(await screen.findByText(message.preview));
    await act(async () => late.resolve(fixture.messages.slice(0, 2)));
    expect(screen.getByText(message.preview).closest("article")).toHaveClass("is-selected");
    expect(screen.queryByText("Delete 2 messages for everyone?")).not.toBeInTheDocument();
    if (_field === "account") expect(screen.queryByText("First lossless target")).not.toBeInTheDocument();
  });

  it.each([
    ["account", fixture.syntheticContext, fixture.messages[2]],
    ["source", otherSource, sourceMessage]
  ] as const)("does not publish stale settings after %s verification advances", async (_field, nextContext, message) => {
    const saved = deferred<unknown>();
    let verified = false;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      const request = (args as { request: { context: unknown } }).request;
      if (command === "save_connection_settings_v2") return saved.promise;
      if (command === "get_connection_settings_v2") return { contractVersion: 2, context: request.context, payload: fixture.connectionSettings };
      if (command === "get_bootstrap_snapshot_v2" && !verified) return { contractVersion: 2, context: null, payload: {
        identity: { state: "pending" }, auth: { schema: "telegram.auth", version: 1, payload: { stage: "ready", hint: null, qrLink: null } },
        catalog: { phase: "idle", total: 0, processed: 0 }, chats: [], recentJobs: [], legacyHistory: []
      } };
      if (command === "get_bootstrap_snapshot_v2" || command === "get_snapshot_v2") {
        const next = wireSnapshot(nextContext);
        next.payload.chats = next.payload.chats.map(chat => ({ ...chat, scope: nextContext.scope }));
        return next;
      }
      if (command === "search_messages_v2") return { contractVersion: 2, context: nextContext, payload: { items: [wireMessage(message)], nextCursor: null } };
      if (command === "get_intents_v2") return { contractVersion: 2, context: nextContext, payload: [] };
      throw new Error(`Unexpected native boundary command: ${command}`);
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Verifying Telegram account" });
    expect(screen.queryByText("Search every chat")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Connection settings" }));
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(vi.mocked(invoke).mock.calls.some(([c]) => c === "save_connection_settings_v2")).toBe(true));
    verified = true;
    fireEvent.click(await screen.findByText(message.preview, {}, { timeout: 2000 }));
    await act(async () => saved.resolve(wireSnapshot(fixture.context)));
    expect(screen.getByText(message.preview).closest("article")).toHaveClass("is-selected");
    expect(screen.queryByText("Connection settings applied.")).not.toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.find(([c]) => c === "save_connection_settings_v2")?.[1]).toMatchObject({ request: { context: null } });
  });

  it.each(["refresh", "poll"])("ignores a stale %s after switching only the source", async operation => {
    const late = deferred<unknown[]>();
    const transport = installTransport({ jobs: [fixture.job], switchContext: otherSource, switchMessages: [sourceMessage],
      poll: operation === "poll" ? () => late.promise : async () => [{ ...fixture.job, status: "completed" }],
      refresh: operation === "refresh" ? () => late.promise : async () => [] });
    render(<App />);
    await screen.findByText("First lossless target");
    await waitFor(() => expect(transport.mock.calls.some(([c]) => c === (operation === "poll" ? "get_jobs_v2" : "refresh_chats_v2"))).toBe(true), { timeout: 2000 });
    fireEvent.click(screen.getByRole("button", { name: /connection settings/i }));
    fireEvent.click(await screen.findByRole("button", { name: "Save settings" }));
    await screen.findByText("Connection settings applied.");
    fireEvent.click(await screen.findByText(sourceMessage.preview));
    await act(async () => late.resolve(operation === "poll" ? [{ ...fixture.job, status: "completed" }] : []));
    expect(screen.getByText(sourceMessage.preview).closest("article")).toHaveClass("is-selected");
    if (operation === "poll") expect(transport.mock.calls.filter(([c]) => c === "refresh_chats_v2")).toEqual([]);
  });
});
