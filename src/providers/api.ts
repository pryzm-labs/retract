import type { RetractApi } from "../api-contract";
import type { AppSnapshot, PlanOperation } from "../types";
import { array, decodeBootstrap, decodeContext, decodeContent, decodeConversation, decodeEnvelope, decodeError, decodeIntents, decodeJob, decodePlan, decodeRefs, invalid, object, nullable, recordRef, text, type IntentDescriptor } from "./contract";
import { contentView, conversationView, decodeSettings, jobView, planView, searchFilters, snapshotView } from "./telegram";
import { refKey, sameScope, type ActiveContext, type ScopedResourceRef } from "./identity";
export type Transport = (command: string, request: unknown) => Promise<unknown>;
export class ProviderApiError extends Error {
  readonly code: ReturnType<typeof decodeError>["code"];
  readonly retryAt: string | null;
  constructor(error: ReturnType<typeof decodeError>) {
    super(error.message);
    this.code = error.code;
    this.retryAt = error.retryAt;
  }
}
export class CommittedSettingsError extends Error {
  readonly snapshot: AppSnapshot;
  constructor(snapshot: AppSnapshot, cause: unknown) {
    super("Settings were applied, but their current view could not be loaded.", { cause });
    this.snapshot = snapshot;
  }
}
export function createApi(transport: Transport, isDesktop: () => boolean): RetractApi {
  async function call(command: string, payload: unknown, context: ActiveContext | null) {
    if (context) decodeContext(context);
    try { return await transport(command, { contractVersion: 2, context, payload }); }
    catch (error) { if (error instanceof Error) throw error; throw new ProviderApiError(decodeError(error)); }
  }
  async function active(command: string, payload: unknown, context: ActiveContext) { decodeContext(context); return decodeEnvelope(await call(command, payload, context), context).payload; }
  async function intents(targets: ScopedResourceRef[], context: ActiveContext): Promise<IntentDescriptor[]> { decodeRefs(targets, context.scope); return decodeIntents(await active("get_intents_v2", { actionId: "", targets, actor: null }, context)); }
  async function prepare(targets: ScopedResourceRef[], operation: PlanOperation, actor: ScopedResourceRef | null, context: ActiveContext) {
    const catalog = await intents(targets, context);
    const selected = catalog.find(i => i.actionId === operation);
    if (!selected || selected.requiresActor !== (actor !== null) || !selected.descriptors.some(d => d.availability === "executable" || d.availability === "live_preflight_required")) return invalid();
    if (actor) decodeRefs([actor], context.scope, "actor");
    return planView(decodePlan(await active("prepare_intent_v2", { actionId: selected.actionId, targets, actor }, context), context.scope), context, operation);
  }
  const api: RetractApi = {
    isDesktop,
    bootstrapSnapshot: async (context = null) => snapshotView(decodeBootstrap(await call("get_bootstrap_snapshot_v2", {}, context), context)),
    snapshot: async context => snapshotView(decodeBootstrap(await call("get_snapshot_v2", {}, context), context)),
    connectionSettings: async context => decodeSettings(decodeEnvelope(await call("get_connection_settings_v2", {}, context), context, true).payload),
    saveConnectionSettings: async (request, context) => {
      const snapshot = snapshotView(decodeBootstrap(await call("save_connection_settings_v2", request, context)));
      try {
        const connectionSettings = await api.connectionSettings(snapshot.context);
        return { snapshot, connectionSettings };
      } catch (cause) { throw new CommittedSettingsError(snapshot, cause); }
    },
    search: async (request, context) => {
      decodeRefs(request.conversations, context.scope, "conversation");
      const p = object(await active("search_messages_v2", { query: request.query, conversations: request.conversations, filters: searchFilters(request), limit: request.limit }, context));
      const records = array(p.items, item => decodeContent(item, context.scope));
      decodeRefs(records.map(recordRef), context.scope, "content");
      const nextCursor = nullable(p.nextCursor, text);
      return { messages: records.map(contentView), returned: records.length, truncated: nextCursor !== null || records.length >= request.limit };
    },
    refreshChats: async (conversations, context) => {
      decodeRefs(conversations, context.scope, "conversation");
      const requested = new Set(conversations.map(refKey));
      const records = array(await active("refresh_chats_v2", { conversations }, context), item => decodeConversation(item, context.scope));
      decodeRefs(records.map(recordRef), context.scope);
      if (records.some(r => !requested.has(refKey(recordRef(r))))) return invalid();
      return records.map(conversationView);
    },
    intents,
    prepareSelection: async (messageRefs, context) => { decodeRefs(messageRefs, context.scope, "content"); return planView(decodePlan(await active("prepare_selection_v2", { messageRefs }, context), context.scope), context); },
    prepareOwnMessages: (chat, context) => prepare([chat], "delete_my_messages", null, context),
    prepareChatAction: (chat, operation, context) => prepare([chat], operation, null, context),
    prepareSenderAction: (chat, actor, context) => prepare([chat], "delete_by_sender", actor, context),
    requestQrAuth: async context => { snapshotView(decodeBootstrap(await call("submit_auth_v2", { operation: "request_qr_auth", value: null }, context), context)); },
    submitAuth: async (operation, value, context) => { snapshotView(decodeBootstrap(await call("submit_auth_v2", { operation, value }, context), context)); },
    retryIdentity: async context => { snapshotView(decodeBootstrap(await call("retry_identity_v2", {}, context), context)); },
    authorizePlan: async plan => { const p = await active("authorize_plan_v2", { planId: plan.id, fingerprint: plan.fingerprint }, plan.context); if (p !== null) object(p); },
    execute: async (plan, irreversibleAcknowledged, typedChatTitle) => {
      const job = decodeJob(await active("start_execution_v2", { planId: plan.id, fingerprint: plan.fingerprint, irreversibleAcknowledged, typedChatTitle: typedChatTitle || null }, plan.context));
      if (job.planId !== plan.id || !sameScope(job.scope, plan.context.scope)) return invalid(); return jobView(job);
    },
    jobs: async context => array(await active("get_jobs_v2", {}, context), decodeJob).map(jobView),
    cancelJob: async (jobId, context) => { const job = decodeJob(await active("cancel_job_v2", { jobId }, context)); if (job.id !== jobId || !sameScope(job.scope, context.scope)) return invalid(); return jobView(job); }
  };
  return api;
}
