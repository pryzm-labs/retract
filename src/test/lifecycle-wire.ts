import { createHash } from "node:crypto";
import fixture from "./fixtures/provider-lifecycle.json";
import { decodeContext, decodeContent, decodeConversation, decodeJob, type ActionDescriptor } from "../providers/contract";
import { contentView, conversationView } from "../providers/telegram";

// Independent test fixture UUIDv5 construction. Expected content/conversation UUIDs
// remain the reviewed literal shared JSON; only auxiliary actor/group refs use this.
function auxiliaryRef(scope: typeof fixture.context.scope, kind: "actor" | "grouping", keys: string[]) {
  const resource = { provider: scope.provider, accountId: scope.accountId, resourceKind: kind, locatorSchema: `${scope.provider}.${kind}`, locatorVersion: 1,
    canonicalKey: JSON.stringify([`${scope.provider}-${kind}-v1`, ...keys]), locatorPayload: { keys } };
  const bytes = createHash("sha1").update(Buffer.from(scope.accountId.replaceAll("-", ""), "hex")).update(JSON.stringify(["retract-resource-v1", kind, resource.locatorSchema, 1, resource.canonicalKey])).digest().subarray(0, 16);
  bytes[6] = (bytes[6] & 15) | 80; bytes[8] = (bytes[8] & 63) | 128;
  const s = bytes.toString("hex"), id = `${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20)}`;
  return { scope, id, resource };
}
export function wireMessage(message: typeof fixture.messages[number]) {
  const actor = auxiliaryRef(message.scope, "actor", [message.senderId]);
  return { id: message.ref.id, scope: message.scope, resource: message.ref.resource, conversationId: message.conversation.id, authorId: actor.id,
    timestamp: message.sentAt, editedAt: null, kind: message.contentKind === "photo" ? "image" : message.contentKind, searchableText: message.preview, attachments: [], replyTo: null, threadParent: null,
    externalLocation: "unsupported", evidence: "live", observedAt: message.observedAt, privacyFindings: message.privacyFindings, detectorVersion: null,
    providerMetadata: { schema: "telegram.content_metadata", version: 1, payload: { originalKind: message.contentKind, outgoing: message.isOutgoing, pinned: message.isPinned, grouping: message.albumId ? auxiliaryRef(message.scope, "grouping", [message.chatId, message.albumId]) : null, actor, senderName: message.senderName, deletionReach: message.deletionReach } } };
}
export function wireChat(chat: typeof fixture.chats[number]) {
  return { id: chat.ref.id, scope: chat.scope, resource: chat.ref.resource, kind: chat.kind === "supergroup" ? "group" : "direct", title: chat.title, parentId: null, participantCount: chat.memberCount, participants: [], evidence: "live", observedAt: "2026-09-01T12:01:00Z",
    providerMetadata: { schema: "telegram.conversation_metadata", version: 1, payload: { originalKind: chat.kind, archived: chat.archived, conversationState: chat.conversationState, capabilities: chat.capabilities, avatarSeed: chat.avatarSeed } } };
}
export const lifecycleContext = decodeContext(fixture.context);
export const lifecycleSyntheticContext = decodeContext(fixture.syntheticContext);
export const lifecycleMessages = fixture.messages.map(m => contentView(decodeContent(wireMessage(m), decodeContext({ scope: m.scope, sessionGeneration: fixture.context.sessionGeneration }).scope)));
export const lifecycleChats = fixture.chats.map(c => conversationView(decodeConversation(wireChat(c), decodeContext({ scope: c.scope, sessionGeneration: fixture.context.sessionGeneration }).scope)));
export function wireJob(job: typeof fixture.job) { return { id: job.id, planId: job.planId, scope: job.scope, dirtyRefs: job.dirtyRefs, status: job.status, counters: { selected: job.total, eligible: job.total, deleted: job.deleted, skipped: job.skipped, failed: job.failed, uncertain: 0 }, nextBatch: job.nextBatch, retryAt: null, diagnostics: [], startedAuthorized: true, createdAt: job.createdAt, updatedAt: job.updatedAt }; }
export const deletionDescriptor: ActionDescriptor = { id: "test.delete", kind: "delete_remote_item", effect: "removed_for_all_participants", availability: "live_preflight_required", unavailableReason: null, requiresLivePreflight: true, batch: { maxTargets: 100, maxParallel: 1 }, confirmationTier: "low", destructive: true, irreversible: true, advisory: null };
export function wirePlan(context: typeof fixture.context, targets: unknown[]) { return { id: fixture.plan.id, scope: context.scope, steps: [{ descriptor: deletionDescriptor, targets }], targets, confirmation: { tier: "low", acknowledgementRequired: true, ownerAuthRequired: true, exactText: null }, recipe: { schema: "test.frozen", version: 1, payload: {} }, restartPolicy: "resume_frozen_targets", createdAt: fixture.plan.createdAt, fingerprint: "sha256-v1:" + "a".repeat(64) }; }
export function wireSnapshot(context = fixture.context, jobs: unknown[] = []) { return { contractVersion: 2, context, payload: { identity: { state: "ready" }, auth: { schema: "telegram.auth", version: 1, payload: { stage: "ready", hint: null, qrLink: null } }, catalog: { phase: "ready", total: 2, processed: 2 }, chats: fixture.chats.filter(c => c.scope.provider === context.scope.provider).map(wireChat), recentJobs: jobs.map(j => wireJob(j as typeof fixture.job)), legacyHistory: [] } }; }
