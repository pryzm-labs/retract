import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { api } from "./api.desktop";
import { messageKey } from "./components/ResultsList";
import fixture from "./test/fixtures/provider-lifecycle.json";
import type { MessageSnapshot } from "./types";

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

function snapshot(context = fixture.context, jobs: unknown[] = []) {
  return {
    contractVersion: 2,
    context,
    runtimeMode: "demo",
    accountLabel: "Synthetic account",
    modeReason: null,
    chats: fixture.chats.filter((chat) => chat.scope.provider === context.scope.provider),
    recentJobs: jobs,
    safetyNotice: "Synthetic data only",
    auth: { stage: "ready", hint: null, qrLink: null }
  };
}

// Transitional flat responses deliberately retain v2 context/ref fields at runtime.
// Task 7 rewires these transport responses to v2; it must retain the assertions.
function installTransport(options: {
  messages?: FixtureMessage[];
  context?: FixtureContext;
  jobs?: unknown[];
  poll?: () => Promise<unknown[]>;
  refresh?: () => Promise<unknown[]>;
} = {}) {
  let currentContext = options.context ?? fixture.context;
  let messages = options.messages ?? fixture.messages.slice(0, 2);
  const transport = vi.mocked(invoke);
  transport.mockImplementation(async (command, args) => {
    switch (command) {
      case "get_bootstrap_snapshot":
      case "get_snapshot": return snapshot(currentContext, options.jobs);
      case "get_connection_settings": return fixture.connectionSettings;
      case "get_auth_snapshot": return { stage: "ready", hint: null, qrLink: null };
      case "get_catalog_progress": return { phase: "ready", total: 2, processed: 2 };
      case "search_messages": {
        const request = (args as Record<string, unknown>)?.request as { contentKinds?: string[] };
        const visible = request.contentKinds?.length ? [] : messages;
        return { messages: visible, returned: visible.length, truncated: false, context: currentContext };
      }
      case "prepare_selection": return {
        ...fixture.plan,
        scope: currentContext.scope,
        summary: { selected: messages.length, deleteForEveryone: messages.length, selfOnly: 0, cannotDelete: 0 }
      };
      case "get_jobs": return options.poll ? options.poll() : [];
      case "refresh_chats": return options.refresh ? options.refresh() : [];
      case "save_connection_settings":
        currentContext = fixture.syntheticContext;
        messages = [fixture.messages[2]];
        return { connectionSettings: fixture.connectionSettings, snapshot: snapshot(currentContext) };
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
    const first = fixture.messages[0];
    const otherAccount = { ...first, scope: fixture.otherScope };
    expect(messageKey(first as unknown as MessageSnapshot))
      .not.toBe(messageKey(otherAccount as unknown as MessageSnapshot));
  });

  it.each([
    ["provider", { ...fixture.context.scope, provider: "synthetic" }],
    ["source", { ...fixture.context.scope, sourceId: "22222222-2222-4222-8222-222222222222" }]
  ])("includes %s in selection identity", (_field, scope) => {
    const first = fixture.messages[0];
    expect(messageKey(first as unknown as MessageSnapshot))
      .not.toBe(messageKey({ ...first, scope } as unknown as MessageSnapshot));
  });

  it("keeps complete same-native records from different providers distinct", () => {
    expect(messageKey(fixture.messages[0] as unknown as MessageSnapshot))
      .not.toBe(messageKey(fixture.sameNativeOtherProviderMessage as unknown as MessageSnapshot));
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
    // Temporary v1 call site only; the desired wire request below is frozen.
    await api.refreshChats([fixture.chats[0].id] as unknown as number[]);
    expect(transport).toHaveBeenLastCalledWith("refresh_chats_v2", { request: {
      contractVersion: 2,
      context: fixture.context,
      payload: { conversationRefs: [fixture.dirtyRefs[0]] }
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
    await waitFor(() => expect(transport.mock.calls.some(([command]) => command === "refresh_chats")).toBe(true), { timeout: 2000 });
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
    await waitFor(() => expect(transport.mock.calls.some(([command]) => command === "get_jobs")).toBe(true), { timeout: 2000 });
    await switchToSyntheticAccount();
    await act(async () => { poll.resolve([{ ...fixture.job, status: "completed" }]); });
    expect(screen.getByText("Current synthetic target").closest("article"),
      "a completed old-account job must not reconcile the new account's matching native chat ID")
      .toHaveClass("is-selected");
    expect(transport.mock.calls.filter(([command]) => command.startsWith("refresh_chats"))).toEqual([]);
  });
});
