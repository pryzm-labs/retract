import { beforeEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { invoke } from "@tauri-apps/api/core";
import { api } from "../api.desktop";
import { createApi } from "./api";
import fixture from "../test/fixtures/provider-lifecycle.json";
import { lifecycleContext, wireMessage, wireJob, wireChat, wirePlan } from "../test/lifecycle-wire";
import { decodeRef, decodeContent, decodeConversation, decodeBootstrap, decodePlan, decodeJob, type ContentRecord, type CommandRequest, type BootstrapResponse, type Page, type ScopedJobRecord } from "./contract";
import type { ActiveContext, ContentId } from "./identity";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const ready = () => ({ contractVersion: 2, context: structuredClone(fixture.context), payload: {
  identity: { state: "ready" }, auth: { schema: "telegram.auth", version: 1, payload: { stage: "ready", hint: null, qrLink: null } },
  catalog: { phase: "ready", total: 0, processed: 0 }, chats: [], recentJobs: [], legacyHistory: []
} });

describe("real desktop v2 response boundary", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it("discovers a verified context through the optional-context bootstrap", async () => {
    vi.mocked(invoke).mockResolvedValue(ready());
    const result = await api.bootstrapSnapshot();
    expect(result).toHaveProperty("context", fixture.context);
    expect(invoke).toHaveBeenCalledWith("get_bootstrap_snapshot_v2", { request: { contractVersion: 2, context: null, payload: {} } });
  });
  it.each([
    ["version", (v: any) => { v.contractVersion = 1; }],
    ["missing context", (v: any) => { delete v.context; }],
    ["unverified ready", (v: any) => { v.context = null; }],
    ["numeric account", (v: any) => { v.context.scope.accountId = 42; }],
    ["malformed generation", (v: any) => { v.context.sessionGeneration = "session"; }],
    ["invalid optional auth", (v: any) => { v.payload.auth = false; }],
    ["invalid auth stage", (v: any) => { v.payload.auth.payload.stage = "invented"; }],
    ["invalid nullable hint", (v: any) => { v.payload.auth.payload.hint = 17; }],
    ["invalid catalog counter", (v: any) => { v.payload.catalog.processed = "0"; }],
    ["numeric conversation ID", (v: any) => { v.payload.chats = [{ id: 42 }]; }],
    ["malformed job", (v: any) => { v.payload.recentJobs = [{ id: "invalid" }]; }],
    ["malformed historical job", (v: any) => { v.payload.legacyHistory = [{ id: 7 }]; }]
  ])("rejects %s before publishing", async (_name, mutate) => {
    const value = ready(); mutate(value);
    vi.mocked(invoke).mockResolvedValue(value);
    await expect(api.bootstrapSnapshot()).rejects.toThrow();
  });
});

describe("record validation through the actual search and job adapters", () => {
  const search = () => api.search({ query: "", conversations: [], chatKinds: [], contentKinds: [], direction: "any", excludePinned: false, limit: 500 }, lifecycleContext);
  const response = () => structuredClone({ contractVersion: 2, context: fixture.context, payload: { items: [wireMessage(fixture.messages[0])], nextCursor: null, completion: "complete" } });
  it("focused creates secure ordered search clients without crypto.randomUUID", async () => {
    const nativeCrypto = globalThis.crypto;
    vi.stubGlobal("crypto", { getRandomValues: nativeCrypto.getRandomValues.bind(nativeCrypto) });
    try {
      const transport = vi.fn(async (_command: string, _request: unknown) => response());
      const first = createApi(transport, () => true);
      const second = createApi(transport, () => true);
      const request = { query: "", conversations: [], chatKinds: [], contentKinds: [], direction: "any" as const, excludePinned: false, limit: 500 };
      await first.search(request, lifecycleContext);
      await first.search(request, lifecycleContext);
      await second.search(request, lifecycleContext);
      const tokens = transport.mock.calls.map(([, raw]) => (raw as { payload: { searchToken: { clientId: string; sequence: number } } }).payload.searchToken);
      expect(tokens[0].clientId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
      expect(tokens[1]).toEqual({ clientId: tokens[0].clientId, sequence: 2 });
      expect(tokens[0].sequence).toBe(1);
      expect(tokens[2].clientId).not.toBe(tokens[0].clientId);
    } finally { vi.unstubAllGlobals(); }
  });
  it.each(["candidate_limit", "incomplete"])("focused preserves %s coverage below the result limit", async completion => {
    vi.mocked(invoke).mockResolvedValue({ ...response(), payload: { ...response().payload, completion } });
    expect(await search()).toMatchObject({ returned: 1, truncated: true, completion });
  });
  it.each([undefined, "unknown", false, null])("focused rejects missing or invalid completion %s", async completion => {
    vi.mocked(invoke).mockResolvedValue({ ...response(), payload: { ...response().payload, completion } });
    await expect(search()).rejects.toThrow("Invalid provider response");
  });

  it("focused sends request-specific cancellation and rejects a late successful response", async () => {
    const controller = new AbortController();
    let finish: (value: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (command: string) => command === "search_messages_v2" ? new Promise(resolve => { finish = resolve; }) : { contractVersion: 2, context: fixture.context, payload: null });
    const request = { query: "", conversations: [], chatKinds: [], contentKinds: [], direction: "any" as const, excludePinned: false, limit: 500 };
    const first = api.search(request, lifecycleContext, controller.signal);
    const rejection = expect(first).rejects.toMatchObject({ code: "search_cancelled" });
    const captured = (vi.mocked(invoke).mock.calls[0][1] as any).request;
    controller.abort();
    expect(invoke).toHaveBeenCalledWith("cancel_search_v2", { request: { contractVersion: 2, context: lifecycleContext, payload: captured.payload.searchToken } });
    finish(response());
    await rejection;
    vi.mocked(invoke).mockResolvedValue(response());
    await api.search(request, lifecycleContext);
    const latest = (vi.mocked(invoke).mock.calls.at(-1)![1] as any).request.payload.searchToken;
    expect(latest.clientId).toBe(captured.payload.searchToken.clientId);
    expect(latest.sequence).toBeGreaterThan(captured.payload.searchToken.sequence);
  });
  it.each(["search_cancelled", "search_incomplete", "search_session_limit"])("focused decodes safe %s diagnostics", async code => {
    vi.mocked(invoke).mockRejectedValue({ code, retryAt: null });
    await expect(search()).rejects.toMatchObject({ code });
  });
  it("retains the independently reviewed UUID and both exact native decimal strings", async () => {
    const value = response(); value.payload.items.push(wireMessage(fixture.messages[1]));
    vi.mocked(invoke).mockResolvedValue(value);
    const results = await search();
    expect(results.messages.map(m => m.messageId)).toEqual(["e8de4fa5-c211-5f5a-bdf1-6f52d06ba26e", "075e4782-bc3a-5469-97e7-f11781240105"]);
    expect(results.messages.map(m => m.ref.resource.locatorPayload)).toEqual([{ chatId: "-1001", messageId: "9007199254740992" }, { chatId: "-1001", messageId: "9007199254740993" }]);
  });
  it.each([
    ["numeric content id", (r: any) => { r.id = 42; }],
    ["scope disagreement", (r: any) => { r.scope = fixture.otherScope; }],
    ["resource account disagreement", (r: any) => { r.resource.accountId = fixture.otherScope.accountId; }],
    ["canonical UUID disagreement", (r: any) => { r.resource.canonicalKey = "different"; }],
    ["unknown locator version", (r: any) => { r.resource.locatorVersion = 2; }],
    ["wrong kind", (r: any) => { r.resource.resourceKind = "actor"; }],
    ["numeric conversation id", (r: any) => { r.conversationId = -1001; }],
    ["numeric author id", (r: any) => { r.authorId = 42; }],
    ["malformed optional attachment", (r: any) => { r.attachments = [{ kind: "image", safeDisplayName: 42 }]; }],
    ["malformed nullable edit", (r: any) => { r.editedAt = false; }],
    ["missing nullable field", (r: any) => { delete r.replyTo; }],
    ["mismatched actor ref", (r: any) => { r.authorId = fixture.plan.id; }],
    ["mismatched actor scope", (r: any) => { r.providerMetadata.payload.actor.scope = fixture.otherScope; }],
    ["malformed grouping", (r: any) => { r.providerMetadata.payload.grouping = 7001; }]
  ])("rejects %s before exposing results", async (_name, mutate) => {
    const value = response(); mutate(value.payload.items[0]);
    vi.mocked(invoke).mockResolvedValue(value);
    await expect(search()).rejects.toThrow();
  });
  it("rejects conflicting payloads for the same application identity", async () => {
    const value = response(), duplicate = structuredClone(value.payload.items[0]);
    duplicate.resource.locatorPayload = { chatId: "-1001", messageId: "123" };
    value.payload.items.push(duplicate);
    vi.mocked(invoke).mockResolvedValue(value);
    await expect(search()).rejects.toThrow();
  });
  it("sends captured context with jobs and rejects malformed counters", async () => {
    const job = wireJob(fixture.job); job.counters.eligible = 1;
    vi.mocked(invoke).mockResolvedValue({ contractVersion: 2, context: fixture.context, payload: [job] });
    await expect(api.jobs(lifecycleContext)).rejects.toThrow();
    expect(invoke).toHaveBeenLastCalledWith("get_jobs_v2", { request: { contractVersion: 2, context: fixture.context, payload: {} } });
  });
  it("accepts content refs as dirty refs for item-level provider jobs", () => {
    const job = wireJob(fixture.job);
    job.dirtyRefs = [wireMessage(fixture.messages[0])];
    expect(decodeJob(job).dirtyRefs[0].resource.resourceKind).toBe("content");
  });
  it("rejects unsolicited targeted refresh records without widening to a catalog", async () => {
    vi.mocked(invoke).mockResolvedValue({ contractVersion: 2, context: fixture.syntheticContext, payload: [] });
    await expect(api.refreshChats([decodeRef(fixture.dirtyRefs[0])], lifecycleContext)).rejects.toThrow();
    expect(invoke).toHaveBeenLastCalledWith("refresh_chats_v2", { request: { contractVersion: 2, context: fixture.context, payload: { conversations: [fixture.dirtyRefs[0]] } } });
  });
});

describe("auth mutations retain the v2 response boundary", () => {
  it.each(["submit", "qr", "retry"])("validates the entire %s bootstrap response", async operation => {
    const value = ready(); value.payload.catalog.processed = -1;
    vi.mocked(invoke).mockResolvedValue(value);
    const result = operation === "submit" ? api.submitAuth("submit_password", "test-only", lifecycleContext)
      : operation === "qr" ? api.requestQrAuth(lifecycleContext) : api.retryIdentity(lifecycleContext);
    await expect(result).rejects.toThrow();
    expect(invoke).toHaveBeenLastCalledWith(operation === "retry" ? "retry_identity_v2" : "submit_auth_v2", { request: {
      contractVersion: 2, context: fixture.context,
      payload: operation === "retry" ? {} : { operation: operation === "qr" ? "request_qr_auth" : "submit_password", value: operation === "qr" ? null : "test-only" }
    } });
  });
  it("retains an unknown author without fabricating an actionable actor ref", async () => {
    const record = wireMessage(fixture.messages[0]);
    const metadata = record.providerMetadata.payload;
    delete (metadata as Partial<typeof metadata>).actor;
    vi.mocked(invoke).mockResolvedValue({ contractVersion: 2, context: fixture.context, payload: { items: [record], nextCursor: null, completion: "complete" } });
    const response = await api.search({ query: "", conversations: [], chatKinds: [], contentKinds: [], direction: "any", excludePinned: false, limit: 500 }, lifecycleContext);
    expect(response.messages[0].actorRef).toBeNull();
  });
});

describe("exact Rust/TypeScript v2 shapes", () => {
  function fields(path: string, name: string) {
    const source = readFileSync(path, "utf8");
    const body = source.match(new RegExp(`pub struct ${name}(?:<[^>]+>)? \\{([\\s\\S]*?)\\n\\}`))?.[1];
    expect(body, `${name} must exist in the real Rust contract`).toBeDefined();
    return [...body!.matchAll(/pub (\w+):/g)].map(match => match[1].replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())).sort();
  }
  it("matches real Rust envelope, identity, record, descriptor, plan and job field names exactly", () => {
    const bootstrap = decodeBootstrap(ready());
    const content = decodeContent(wireMessage(fixture.messages[0]), lifecycleContext.scope);
    const conversation = decodeConversation(wireChat(fixture.chats[0]), lifecycleContext.scope);
    const plan = decodePlan(wirePlan(fixture.context, [fixture.messages[0].ref]), lifecycleContext.scope);
    const job = decodeJob(wireJob(fixture.job));
    const ref = decodeRef(fixture.messages[0].ref);
    const shapes: Array<[string, string, object]> = [
      ["src-tauri/src/compatibility/model_v2.rs", "BootstrapResponse", bootstrap],
      ["src-tauri/src/compatibility/model_v2.rs", "BootstrapSnapshot", bootstrap.payload],
      ["src-tauri/src/compatibility/model_v2.rs", "CatalogProgress", bootstrap.payload.catalog],
      ["crates/retract-domain/src/identity.rs", "Scope", lifecycleContext.scope],
      ["crates/retract-domain/src/identity.rs", "ActiveContext", lifecycleContext],
      ["crates/retract-domain/src/identity.rs", "ScopedResourceRef", ref],
      ["crates/retract-domain/src/identity.rs", "ProviderResourceRef", ref.resource],
      ["crates/retract-domain/src/records.rs", "ContentRecord", content],
      ["crates/retract-domain/src/records.rs", "ConversationRecord", conversation],
      ["crates/retract-domain/src/actions.rs", "ActionDescriptor", plan.steps[0].descriptor],
      ["crates/retract-domain/src/plans.rs", "RemediationPlan", plan],
      ["crates/retract-domain/src/plans.rs", "ActionStep", plan.steps[0]],
      ["crates/retract-domain/src/plans.rs", "ConfirmationRequirements", plan.confirmation],
      ["crates/retract-domain/src/plans.rs", "ScopedJobRecord", job],
      ["crates/retract-domain/src/plans.rs", "JobCounters", job.counters],
      ["src-tauri/src/providers/ports.rs", "SearchPage", { items: [], nextCursor: null, completion: "complete" }]
    ];
    for (const [path, name, value] of shapes) expect(Object.keys(value).sort(), name).toEqual(fields(path, name));
  });
  it("keeps UUID brands, nullable fields and captured-context requests statically exact", () => {
    expectTypeOf<ContentRecord["id"]>().toEqualTypeOf<ContentId>();
    expectTypeOf<ContentRecord["id"]>().not.toEqualTypeOf<number>();
    expectTypeOf<ContentRecord["editedAt"]>().toEqualTypeOf<string | null>();
    expectTypeOf<ContentRecord["replyTo"]>().toEqualTypeOf<ContentId | null>();
    expectTypeOf<Page<ContentRecord>>().toEqualTypeOf<{ items: ContentRecord[]; nextCursor: string | null }>();
    expectTypeOf<import("./contract").SearchPage<ContentRecord>>().toEqualTypeOf<{ items: ContentRecord[]; nextCursor: string | null; completion: "complete" | "result_limit" | "candidate_limit" | "incomplete" }>();
    expectTypeOf<CommandRequest<{ jobId: ScopedJobRecord["id"] }>>().toEqualTypeOf<{ contractVersion: 2; context: ActiveContext; payload: { jobId: ScopedJobRecord["id"] } }>();
    expectTypeOf<BootstrapResponse<unknown>["context"]>().toEqualTypeOf<ActiveContext | null>();
  });
});
