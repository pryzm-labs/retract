import { providerKey, resourceId, sameContext, sameScope, uuid, type ActiveContext, type Scope, type ScopedResourceRef, type ProviderResourceRef, type ResourceKind, type Uuid, type ConversationId, type ContentId, type ActorId } from "./identity";
export type { ActiveContext, Scope, ScopedResourceRef } from "./identity";
export interface VersionedPayload { schema: string; version: number; payload: unknown }
export interface CommandRequest<T> { contractVersion: 2; context: ActiveContext; payload: T }
export interface CommandResponse<T> { contractVersion: 2; context: ActiveContext; payload: T }
export interface BootstrapResponse<T> { contractVersion: 2; context: ActiveContext | null; payload: T }
export interface Page<T> { items: T[]; nextCursor: string | null }
export type Evidence = "live" | "archive" | "live_and_archive";
export type NormalContentKind = "text" | "image" | "video" | "document" | "voice" | "audio" | "animation" | "sticker" | "poll" | "location" | "contact" | "service" | "other";
export interface ActorRecord { id: ActorId; scope: Scope; resource: ProviderResourceRef; displayName: string; username: string | null; avatar: VersionedPayload | null; evidence: Evidence; observedAt: string }
export interface ConversationRecord {
  id: ConversationId; scope: Scope; resource: ProviderResourceRef; kind: "direct" | "group" | "community_channel" | "broadcast" | "public_thread" | "private_thread" | "account_feed" | "other";
  title: string; parentId: ConversationId | null; participantCount: number | null; participants: ActorRecord[]; evidence: Evidence; observedAt: string; providerMetadata: VersionedPayload | null;
}
export interface AttachmentRecord { kind: NormalContentKind; safeDisplayName: string | null; sizeBytes: number | null; mimeType: string | null; locator: VersionedPayload }
export interface ContentRecord {
  id: ContentId; scope: Scope; conversationId: ConversationId; resource: ProviderResourceRef; authorId: ActorId; timestamp: string; editedAt: string | null; kind: NormalContentKind;
  searchableText: string; attachments: AttachmentRecord[]; replyTo: ContentId | null; threadParent: ConversationId | null;
  externalLocation: "available" | "unavailable" | "unsupported"; evidence: Evidence; observedAt: string; privacyFindings: import("../types").SensitiveDataKind[]; detectorVersion: string | null; providerMetadata: VersionedPayload | null;
}
export type ConfirmationTier = "low" | "medium" | "high" | "critical";
export type ActionKind = "delete_remote_item" | "remove_for_current_account" | "clear_conversation" | "leave_conversation" | "delete_conversation" | "delete_by_actor" | "open_externally" | "manual_remediation" | "remove_local_import";
export type ExpectedEffect = "removed_for_all_participants" | "removed_for_current_account_only" | "public_content_removed" | "membership_removed" | "container_destroyed" | "local_import_removed" | "manual_or_unknown";
export interface ActionDescriptor {
  id: string; kind: ActionKind; effect: ExpectedEffect; availability: "executable" | "live_preflight_required" | "manual_only" | "unavailable"; unavailableReason: SafeError | null;
  requiresLivePreflight: boolean; batch: { maxTargets: number; maxParallel: number }; confirmationTier: ConfirmationTier; destructive: boolean; irreversible: boolean;
  advisory: { costBearing: boolean; rateLimited: boolean } | null;
}
export interface IntentDescriptor { actionId: string; label: string; requiresActor: boolean; descriptors: ActionDescriptor[] }
export interface RemediationPlan {
  id: Uuid; scope: Scope; steps: Array<{ descriptor: ActionDescriptor; targets: ScopedResourceRef[] }>; targets: ScopedResourceRef[];
  confirmation: { tier: ConfirmationTier; acknowledgementRequired: boolean; ownerAuthRequired: boolean; exactText: string | null };
  recipe: VersionedPayload; restartPolicy: "requires_new_review" | "resume_frozen_targets"; createdAt: string; fingerprint: string;
}
export type JobStatus = "queued" | "running" | "blocked" | "completed" | "partial" | "failed" | "cancelled";
export interface ScopedJobRecord {
  id: Uuid; planId: Uuid; scope: Scope; dirtyRefs: ScopedResourceRef[]; status: JobStatus;
  counters: { selected: number; eligible: number; deleted: number; skipped: number; failed: number; uncertain: number };
  nextBatch: number; retryAt: string | null; diagnostics: SafeError[]; startedAuthorized: boolean; createdAt: string; updatedAt: string;
}
export interface LegacyHistoryRecord { id: Uuid; planId: Uuid; operation: import("../types").PlanOperation; status: "completed" | "partial" | "failed" | "cancelled"; total: number; deleted: number; skipped: number; failed: number; nextBatch: number; diagnostics: SafeError[]; createdAt: string; updatedAt: string }
export type IdentityStatus = { state: "unavailable" | "pending" | "ready" } | { state: "failed"; diagnostic: SafeError };
export interface BootstrapSnapshot { identity: IdentityStatus; auth: VersionedPayload | null; catalog: { phase: string; total: number; processed: number }; chats: ConversationRecord[]; recentJobs: ScopedJobRecord[]; legacyHistory: LegacyHistoryRecord[] }
export const safeMessages = {
  authentication_required: "Sign in to continue.", permission_changed: "Permission to perform this action has changed.", not_found: "The requested resource could not be found.", already_removed: "The resource has already been removed.", rate_limited: "The provider requires a wait before continuing.", cost_limit_reached: "The configured cost limit has been reached.", transient: "The provider is temporarily unavailable.", permanent: "The provider could not complete this action.", ambiguous_outcome: "The action outcome is uncertain. Review before retrying.", unsupported_schema: "This data version is not supported.", invalid_archive: "The archive could not be validated.", unsupported_contract_version: "Reload the application to use the current interface.", scope_mismatch: "This action belongs to a different account or source.", stale_context: "The connection has changed. Review this action again.", identity_unavailable: "The account identity could not be verified.", profile_in_use: "This profile is already in use by another application process.", state_persistence_failed: "Progress could not be saved. No further actions were scheduled.", migration_requires_new_review: "Legacy history is preserved. A new review is required.", restart_requires_new_review: "This interrupted action requires a new review."
} as const;
export interface SafeError { code: keyof typeof safeMessages; message: string; retryAt: string | null }
export const invalid = (): never => { throw new Error("Invalid provider response. Reload or retry the connection."); };
export function object(value: unknown): Record<string, unknown> { if (!value || typeof value !== "object" || Array.isArray(value)) return invalid(); return value as Record<string, unknown>; }
export function text(value: unknown): string { if (typeof value !== "string") return invalid(); return value; }
export function bounded(value: unknown, max = 4096): string { const s = text(value); if (!s.trim() || s.length > max || /[\x00-\x1f\x7f]/.test(s)) return invalid(); return s; }
export function count(value: unknown, max = Number.MAX_SAFE_INTEGER): number { if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > max) return invalid(); return value; }
export function bool(value: unknown): boolean { if (typeof value !== "boolean") return invalid(); return value; }
export function array<T>(value: unknown, decode: (v: unknown) => T): T[] { if (!Array.isArray(value) || value.length > 100_000) return invalid(); return value.map(decode); }
export function nullable<T>(value: unknown, decode: (v: unknown) => T): T | null { return value === null ? null : decode(value); }
export function optionalNullable<T>(value: unknown, decode: (v: unknown) => T): T | null { return value === undefined || value === null ? null : decode(value); }
export function choice<const T extends readonly string[]>(value: unknown, values: T): T[number] { const s = text(value); if (!values.includes(s)) return invalid(); return s; }
export function date(value: unknown): string { const s = text(value); if (!/^\d{4}-\d\d-\d\dT.*(?:Z|[+-]\d\d:\d\d)$/.test(s) || !Number.isFinite(Date.parse(s))) return invalid(); return s; }
export function json(value: unknown): unknown { if (value === null || typeof value === "string" || typeof value === "boolean") return value; if (typeof value === "number" && Number.isFinite(value)) return value; if (Array.isArray(value)) return array(value, json); const v = object(value); return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, json(x)])); }
export function decodeScope(value: unknown): Scope { const v = object(value); return { provider: providerKey(v.provider), accountId: uuid<"AccountId">(v.accountId), sourceId: uuid<"SourceId">(v.sourceId) }; }
export function decodeContext(value: unknown): ActiveContext { const v = object(value); return { scope: decodeScope(v.scope), sessionGeneration: uuid(v.sessionGeneration) }; }
export function decodeVersioned(value: unknown): VersionedPayload { const v = object(value); const version = count(v.version, 65535); if (!version) return invalid(); return { schema: bounded(v.schema, 128), version, payload: json(v.payload) }; }
export function decodeRef(value: unknown, expected?: Scope, kind?: ResourceKind): ScopedResourceRef {
  const v = object(value), scope = decodeScope(v.scope), r = object(v.resource);
  const resource: ProviderResourceRef = { provider: providerKey(r.provider), accountId: uuid<"AccountId">(r.accountId), resourceKind: choice(r.resourceKind, ["conversation", "content", "actor", "grouping"]), locatorSchema: bounded(r.locatorSchema, 128), locatorVersion: count(r.locatorVersion, 65535), canonicalKey: bounded(r.canonicalKey, 16384), locatorPayload: json(r.locatorPayload) };
  const id = uuid<"ResourceId">(v.id);
  if (!resource.locatorVersion || resource.provider !== scope.provider || resource.accountId !== scope.accountId || (expected && !sameScope(scope, expected)) || (kind && resource.resourceKind !== kind) || resourceId(resource) !== id) return invalid();
  return { scope, id, resource };
}
export function decodeRefs(value: unknown, expected?: Scope, kind?: ResourceKind): ScopedResourceRef[] {
  const seen = new Map<string, string>();
  return array(value, v => { const ref = decodeRef(v, expected, kind), encoded = JSON.stringify(ref.resource); if (seen.has(ref.id) && seen.get(ref.id) !== encoded) return invalid(); seen.set(ref.id, encoded); return ref; });
}
export function recordRef(record: { scope: Scope; id: string; resource: ProviderResourceRef }): ScopedResourceRef { return decodeRef(record); }
const evidence = (v: unknown) => choice(v, ["live", "archive", "live_and_archive"]);
const contentKind = (v: unknown) => choice(v, ["text", "image", "video", "document", "voice", "audio", "animation", "sticker", "poll", "location", "contact", "service", "other"]);
export const privacyKind = (v: unknown) => choice(v, ["email_address", "phone_number", "postal_address", "precise_location", "personal_identifier", "identity_document", "financial_account", "crypto_wallet", "credential_or_secret", "network_address", "contact_card"]);
export function decodeActor(value: unknown, scope: Scope): ActorRecord { const v = object(value), ref = decodeRef(v, scope, "actor"); return { ...ref, id: uuid<"ActorId">(ref.id), displayName: text(v.displayName), username: nullable(v.username, text), avatar: nullable(v.avatar, decodeVersioned), evidence: evidence(v.evidence), observedAt: date(v.observedAt) }; }
export function decodeConversation(value: unknown, scope: Scope): ConversationRecord { const v = object(value), ref = decodeRef(v, scope, "conversation"); return { ...ref, id: uuid<"ConversationId">(ref.id), kind: choice(v.kind, ["direct", "group", "community_channel", "broadcast", "public_thread", "private_thread", "account_feed", "other"]), title: text(v.title), parentId: nullable(v.parentId, uuid<"ConversationId">), participantCount: nullable(v.participantCount, count), participants: array(v.participants, a => decodeActor(a, scope)), evidence: evidence(v.evidence), observedAt: date(v.observedAt), providerMetadata: nullable(v.providerMetadata, decodeVersioned) }; }
export function decodeContent(value: unknown, scope: Scope): ContentRecord {
  const v = object(value), ref = decodeRef(v, scope, "content");
  const result: ContentRecord = { ...ref, id: uuid<"ContentId">(ref.id), conversationId: uuid<"ConversationId">(v.conversationId), authorId: uuid<"ActorId">(v.authorId), timestamp: date(v.timestamp), editedAt: nullable(v.editedAt, date), kind: contentKind(v.kind), searchableText: text(v.searchableText), attachments: array(v.attachments, a => { const x = object(a); return { kind: contentKind(x.kind), safeDisplayName: nullable(x.safeDisplayName, text), sizeBytes: nullable(x.sizeBytes, count), mimeType: nullable(x.mimeType, text), locator: decodeVersioned(x.locator) }; }), replyTo: nullable(v.replyTo, uuid<"ContentId">), threadParent: nullable(v.threadParent, uuid<"ConversationId">), externalLocation: choice(v.externalLocation, ["available", "unavailable", "unsupported"]), evidence: evidence(v.evidence), observedAt: date(v.observedAt), privacyFindings: array(v.privacyFindings, privacyKind), detectorVersion: nullable(v.detectorVersion, bounded), providerMetadata: nullable(v.providerMetadata, decodeVersioned) };
  if (result.privacyFindings.length && !result.detectorVersion) return invalid(); return result;
}
export function decodeError(value: unknown): SafeError { const v = object(value), code = choice(v.code, Object.keys(safeMessages) as Array<keyof typeof safeMessages>); if (v.message != null && v.message !== safeMessages[code]) return invalid(); return { code, message: safeMessages[code], retryAt: optionalNullable(v.retryAt, date) }; }
export function decodeDescriptor(value: unknown): ActionDescriptor {
  const v = object(value), batch = object(v.batch);
  const result: ActionDescriptor = { id: bounded(v.id, 128), kind: choice(v.kind, ["delete_remote_item", "remove_for_current_account", "clear_conversation", "leave_conversation", "delete_conversation", "delete_by_actor", "open_externally", "manual_remediation", "remove_local_import"]), effect: choice(v.effect, ["removed_for_all_participants", "removed_for_current_account_only", "public_content_removed", "membership_removed", "container_destroyed", "local_import_removed", "manual_or_unknown"]), availability: choice(v.availability, ["executable", "live_preflight_required", "manual_only", "unavailable"]), unavailableReason: nullable(v.unavailableReason, decodeError), requiresLivePreflight: bool(v.requiresLivePreflight), batch: { maxTargets: count(batch.maxTargets, 4294967295), maxParallel: count(batch.maxParallel, 65535) }, confirmationTier: choice(v.confirmationTier, ["low", "medium", "high", "critical"]), destructive: bool(v.destructive), irreversible: bool(v.irreversible), advisory: nullable(v.advisory, a => { const x = object(a); return { costBearing: bool(x.costBearing), rateLimited: bool(x.rateLimited) }; }) };
  if (!result.batch.maxTargets || !result.batch.maxParallel || (result.availability === "unavailable") !== !!result.unavailableReason || (result.availability === "live_preflight_required" && !result.requiresLivePreflight) || (result.effect === "container_destroyed" && (result.confirmationTier !== "critical" || !result.destructive || !result.irreversible))) return invalid(); return result;
}
export function decodeIntents(value: unknown): IntentDescriptor[] { const seen = new Set<string>(); const result = array(value, item => { const v = object(item), actionId = bounded(v.actionId, 128), descriptors = array(v.descriptors, decodeDescriptor); if (seen.has(actionId) || !descriptors.length || descriptors.length > 1000) return invalid(); seen.add(actionId); return { actionId, label: bounded(v.label, 256), requiresActor: bool(v.requiresActor), descriptors }; }); if (result.length > 1000) return invalid(); return result; }
export function decodePlan(value: unknown, expected: Scope): RemediationPlan {
  const v = object(value), scope = decodeScope(v.scope), c = object(v.confirmation);
  if (!sameScope(scope, expected)) return invalid();
  const plan: RemediationPlan = { id: uuid(v.id), scope, steps: array(v.steps, s => { const x = object(s); return { descriptor: decodeDescriptor(x.descriptor), targets: decodeRefs(x.targets, scope) }; }), targets: decodeRefs(v.targets, scope), confirmation: { tier: choice(c.tier, ["low", "medium", "high", "critical"]), acknowledgementRequired: bool(c.acknowledgementRequired), ownerAuthRequired: bool(c.ownerAuthRequired), exactText: nullable(c.exactText, bounded) }, recipe: decodeVersioned(v.recipe), restartPolicy: choice(v.restartPolicy, ["requires_new_review", "resume_frozen_targets"]), createdAt: date(v.createdAt), fingerprint: text(v.fingerprint) };
  if (!/^sha256-v1:[a-f0-9]{64}$/.test(plan.fingerprint) || !plan.steps.length || plan.steps.length > 1000 || !plan.targets.length || !plan.confirmation.acknowledgementRequired || !plan.confirmation.ownerAuthRequired || (["high", "critical"].includes(plan.confirmation.tier) && !plan.confirmation.exactText)) return invalid();
  const targets = new Set(plan.targets.map(r => r.id));
  const steps = new Set(plan.steps.flatMap(step => { if (!step.targets.length || !["executable", "live_preflight_required"].includes(step.descriptor.availability)) return invalid(); return step.targets.map(r => r.id); }));
  decodeRefs([...plan.targets, ...plan.steps.flatMap(s => s.targets)], scope);
  if (targets.size !== steps.size || [...targets].some(id => !steps.has(id))) return invalid(); return plan;
}
export function decodeJob(value: unknown): ScopedJobRecord {
  const v = object(value), scope = decodeScope(v.scope), c = object(v.counters);
  const job: ScopedJobRecord = { id: uuid(v.id), planId: uuid(v.planId), scope, dirtyRefs: decodeRefs(v.dirtyRefs, scope, "conversation"), status: choice(v.status, ["queued", "running", "blocked", "completed", "partial", "failed", "cancelled"]), counters: { selected: count(c.selected), eligible: count(c.eligible), deleted: count(c.deleted), skipped: count(c.skipped), failed: count(c.failed), uncertain: count(c.uncertain) }, nextBatch: count(v.nextBatch), retryAt: nullable(v.retryAt, date), diagnostics: array(v.diagnostics, decodeError), startedAuthorized: bool(v.startedAuthorized), createdAt: date(v.createdAt), updatedAt: date(v.updatedAt) };
  const active = job.status === "queued" || job.status === "running";
  if (Date.parse(job.updatedAt) < Date.parse(job.createdAt) || (!active && job.retryAt) || (["running", "completed", "partial"].includes(job.status) && !job.startedAuthorized) || job.counters.eligible > job.counters.selected || job.counters.skipped > job.counters.selected || job.counters.deleted + job.counters.failed + job.counters.uncertain > job.counters.eligible || (job.status === "completed" && (job.counters.failed || job.counters.uncertain)) || (active && job.counters.uncertain) || (job.status === "blocked" && !job.diagnostics.some(d => ["identity_unavailable", "scope_mismatch", "stale_context"].includes(d.code)))) return invalid(); return job;
}
export function decodeLegacy(value: unknown): LegacyHistoryRecord { const v = object(value); return { id: uuid(v.id), planId: uuid(v.planId), operation: choice(v.operation, ["selected_messages", "delete_my_messages", "clear_history", "clear_history_and_leave", "delete_all_messages_and_leave", "remove_chat_for_self", "delete_by_sender", "delete_group", "leave_chat"]), status: choice(v.status, ["completed", "partial", "failed", "cancelled"]), total: count(v.total), deleted: count(v.deleted), skipped: count(v.skipped), failed: count(v.failed), nextBatch: count(v.nextBatch), diagnostics: array(v.diagnostics, decodeError), createdAt: date(v.createdAt), updatedAt: date(v.updatedAt) }; }
export function decodeBootstrap(value: unknown, expected?: ActiveContext | null): BootstrapResponse<BootstrapSnapshot> {
  const envelope = decodeEnvelope(value, expected, true), v = object(envelope.payload), i = object(v.identity), state = choice(i.state, ["unavailable", "pending", "ready", "failed"]), catalog = object(v.catalog);
  if ((state === "ready") !== (envelope.context !== null)) return invalid();
  const payload: BootstrapSnapshot = { identity: state === "failed" ? { state, diagnostic: decodeError(i.diagnostic) } : { state }, auth: nullable(v.auth, decodeVersioned), catalog: { phase: choice(catalog.phase, ["idle", "discovering", "loading", "ready"]), total: count(catalog.total), processed: count(catalog.processed) }, chats: array(v.chats, c => { if (!envelope.context) return invalid(); return decodeConversation(c, envelope.context.scope); }), recentJobs: array(v.recentJobs, decodeJob), legacyHistory: array(v.legacyHistory, decodeLegacy) };
  if (payload.catalog.processed > payload.catalog.total && payload.catalog.total !== 0) return invalid();
  decodeRefs(payload.chats.map(recordRef), envelope.context?.scope);
  return { contractVersion: 2, context: envelope.context, payload };
}
export function decodeEnvelope(value: unknown, expected?: ActiveContext | null, bootstrap = false): BootstrapResponse<unknown> {
  const v = object(value); if (v.contractVersion !== 2 || !("payload" in v)) return invalid();
  const context = nullable(v.context, decodeContext); if ((!bootstrap && !context) || (expected && !sameContext(context, expected))) return invalid();
  return { contractVersion: 2, context, payload: v.payload };
}
