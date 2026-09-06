// Telegram-owned display/filter projection. Native locatorPayload is always opaque.
import type { AppSnapshot, AuthSnapshot, ChatSummary, MessageSnapshot, PlanView, SearchRequest, PlanOperation, ConnectionSettings, JobRecord } from "../types";
import { array, bool, bounded, choice, count, invalid, nullable, object, optionalNullable, recordRef, text, decodeRef, type BootstrapResponse, type BootstrapSnapshot, type ContentRecord, type ConversationRecord, type RemediationPlan, type ScopedJobRecord, type VersionedPayload } from "./contract";
import { sameScope, type ActiveContext } from "./identity";
const operations = ["selected_messages", "delete_my_messages", "clear_history", "clear_history_and_leave", "delete_all_messages_and_leave", "remove_chat_for_self", "delete_by_sender", "delete_group", "leave_chat"] as const;
function metadata(value: VersionedPayload | null, schema: string) { if (!value || value.schema !== schema || value.version !== 1) return invalid(); return object(value.payload); }
export function authView(value: VersionedPayload | null): AuthSnapshot {
  if (!value) return { stage: "initializing", hint: null, qrLink: null };
  const v = metadata(value, "telegram.auth");
  return { stage: choice(v.stage, ["initializing", "waiting_for_phone", "waiting_for_email_address", "waiting_for_email_code", "waiting_for_code", "waiting_for_password", "waiting_for_other_device", "ready", "logging_out", "closed", "error"]), hint: optionalNullable(v.hint, text), qrLink: optionalNullable(v.qrLink, text) };
}
export function conversationView(record: ConversationRecord): ChatSummary {
  const m = metadata(record.providerMetadata, "telegram.conversation_metadata"), c = object(m.capabilities);
  return { id: record.id, scope: record.scope, ref: recordRef(record), intents: [], title: record.title,
    kind: choice(m.originalKind, ["direct", "basic_group", "supergroup", "channel", "secret"]), archived: bool(m.archived), memberCount: record.participantCount,
    conversationState: choice(m.conversationState, ["empty", "never_replied", "awaiting_reply", "active", "unknown"]), avatarSeed: count(m.avatarSeed, 255),
    capabilities: { role: choice(c.role, ["owner", "admin_with_delete", "admin_limited", "member"]), canDeleteOthers: bool(c.canDeleteOthers), canClearForEveryone: bool(c.canClearForEveryone), canRemoveForSelf: bool(c.canRemoveForSelf), canDeleteGroup: bool(c.canDeleteGroup), canDeleteBySender: bool(c.canDeleteBySender), canLeaveChat: bool(c.canLeaveChat) }
  };
}
export function contentView(record: ContentRecord): MessageSnapshot {
  const m = metadata(record.providerMetadata, "telegram.content_metadata");
  const grouping = nullable(m.grouping, value => decodeRef(value, record.scope, "grouping"));
  const actorRef = optionalNullable(m.actor, value => decodeRef(value, record.scope, "actor"));
  if (actorRef && actorRef.id !== (record.authorId as string)) return invalid();
  return { scope: record.scope, ref: recordRef(record), actorRef, chatId: record.conversationId, messageId: record.id, senderId: record.authorId, senderName: text(m.senderName), sentAt: record.timestamp,
    isOutgoing: bool(m.outgoing), contentKind: choice(m.originalKind, ["text", "photo", "video", "file", "voice", "audio", "animation", "sticker", "poll", "location", "contact", "service", "other"]), preview: record.searchableText,
    privacyFindings: record.privacyFindings, albumId: grouping?.id ?? null, isPinned: bool(m.pinned), deletionReach: choice(m.deletionReach, ["everyone", "self_only", "none"]) };
}
export function snapshotView(response: BootstrapResponse<BootstrapSnapshot>): AppSnapshot {
  const p = response.payload;
  return { context: response.context, identity: p.identity, catalog: { ...p.catalog, phase: choice(p.catalog.phase, ["idle", "discovering", "loading", "ready"]) }, legacyHistory: p.legacyHistory,
    accountLabel: "Telegram account", modeReason: null, safetyNotice: "Self-only deletion is never a fallback.", auth: authView(p.auth), chats: p.chats.map(conversationView), recentJobs: p.recentJobs.map(jobView) };
}
export function jobView(job: ScopedJobRecord): JobRecord { return { ...job, total: job.counters.eligible, deleted: job.counters.deleted, skipped: job.counters.skipped, failed: job.counters.failed, retryAfterSeconds: job.retryAt === null ? null : Math.max(0, Math.ceil((Date.parse(job.retryAt) - Date.now()) / 1000)), errorCodes: job.diagnostics.map(d => d.code) }; }
export function planView(plan: RemediationPlan, context: ActiveContext, fallback: PlanOperation = "selected_messages"): PlanView {
  let operation = fallback, chatTitle = plan.confirmation.exactText, targetSenderName: string | null = null;
  let summary = { selected: plan.targets.filter(r => r.resource.resourceKind === "content").length, deleteForEveryone: plan.targets.filter(r => r.resource.resourceKind === "content").length, selfOnly: 0, cannotDelete: 0 };
  if (plan.recipe.schema === "telegram.compatibility_recipe") {
    const m = metadata(plan.recipe, "telegram.compatibility_recipe");
    operation = choice(m.operation, operations); chatTitle = nullable(m.conversationConfirmationTitle, text); targetSenderName = nullable(m.actorConfirmationName, text);
    const reaches = array(m.items, item => choice(object(item).reach, ["everyone", "self_only", "none"]));
    summary = { selected: reaches.length, deleteForEveryone: reaches.filter(r => r === "everyone").length, selfOnly: reaches.filter(r => r === "self_only").length, cannotDelete: reaches.filter(r => r === "none").length };
  }
  if (!sameScope(plan.scope, context.scope)) return invalid();
  return { ...plan, context, operation, chatTitle, targetSenderName, summary, confirmationTier: plan.confirmation.tier };
}
export function searchFilters(request: SearchRequest): VersionedPayload { return { schema: "telegram.search_filters", version: 1, payload: { chatKinds: request.chatKinds, contentKinds: request.contentKinds, direction: request.direction, minDate: request.minDate ?? null, maxDate: request.maxDate ?? null, excludePinned: request.excludePinned, privacyScan: request.privacyScan ?? false } }; }
export function decodeSettings(value: unknown): ConnectionSettings { const v = object(value); return { setupComplete: bool(v.setupComplete), tdlibPath: text(v.tdlibPath), detectedTdlibPath: optionalNullable(v.detectedTdlibPath, text), bundledTdlibAvailable: bool(v.bundledTdlibAvailable), apiId: optionalNullable(v.apiId, count), apiHashConfigured: bool(v.apiHashConfigured), useTestDc: bool(v.useTestDc), environmentOverrides: array(v.environmentOverrides, text), configurationError: optionalNullable(v.configurationError, text), supportedTdlibVersion: bounded(v.supportedTdlibVersion, 64) }; }
