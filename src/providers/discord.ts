import type { AppSnapshot, ChatSummary, DiscordBrowser, DiscordImportProgress, DiscordImportStatus, DiscordSessionStatus, DiscordSource, MessageSnapshot, SearchRequest } from "../types";
import { array, bool, choice, count, date, decodeScope, nullable, object, optionalNullable, recordRef, text, type BootstrapResponse, type BootstrapSnapshot, type ContentRecord, type ConversationRecord, type VersionedPayload } from "./contract";
import type { ActiveContext } from "./identity";
import { jobView } from "./telegram";

function metadata(value: VersionedPayload | null, schema: string) {
  if (!value || value.schema !== schema || value.version !== 1) throw new Error("Invalid Discord archive metadata.");
  return object(value.payload);
}

const kindMap = { text: "text", image: "photo", video: "video", document: "file", voice: "voice", audio: "audio", animation: "animation", sticker: "sticker", poll: "poll", location: "location", contact: "contact", service: "service", other: "other" } as const;

export function discordConversationView(record: ConversationRecord, accountLabel = ""): ChatSummary {
  const m = metadata(record.providerMetadata, "discord.conversation_metadata");
  const verified = choice(m.verifiedKind, ["direct", "group_direct", "guild_channel", "other"]);
  const guild = optionalNullable(m.guild, value => { const item = object(value); return { id: text(item.id), name: text(item.name) }; });
  const recipients = optionalNullable(m.recipients, value => array(value, text));
  const visibleRecipients = (recipients ?? []).filter(value => value.trim() && value !== accountLabel);
  const category = guild ? "server" : recipients ? "direct" : "other";
  const title = record.title.trim() || (category === "direct"
    ? visibleRecipients.join(", ") || "Unnamed direct message"
    : category === "server" ? "Unnamed channel" : "Unknown Discord chat");
  const detail = category === "server" ? guild!.name
    : category === "direct" ? (visibleRecipients.length > 1 ? "Group direct message" : "Direct message")
      : "Unclassified chat";
  return {
    id: record.id, scope: record.scope, ref: recordRef(record), intents: [], title,
    kind: verified === "direct" ? "direct" : verified === "guild_channel" ? "channel" : "basic_group",
    archived: true, memberCount: record.participantCount, conversationState: "active", avatarSeed: 0,
    capabilities: { role: "member", canDeleteOthers: false, canClearForEveryone: false, canRemoveForSelf: false, canDeleteGroup: false, canDeleteBySender: false, canLeaveChat: false },
    discordNavigation: { category, groupId: guild?.id ?? null, groupLabel: guild?.name ?? null, detail }
  };
}

export function discordContentView(record: ContentRecord): MessageSnapshot {
  metadata(record.providerMetadata, "discord.content_metadata");
  return {
    scope: record.scope, ref: recordRef(record), actorRef: null, chatId: record.conversationId,
    messageId: record.id, senderId: record.authorId, senderName: "You", sentAt: record.timestamp,
    isOutgoing: true, contentKind: kindMap[record.kind], preview: record.searchableText,
    privacyFindings: record.privacyFindings, albumId: null, isPinned: false, deletionReach: "everyone"
  };
}

export function discordSnapshotView(response: BootstrapResponse<BootstrapSnapshot>): AppSnapshot {
  const p = response.payload;
  const account = metadata(p.auth, "discord.account");
  const accountLabel = text(account.accountLabel);
  return { context: response.context, identity: p.identity, catalog: { ...p.catalog, phase: choice(p.catalog.phase, ["idle", "discovering", "loading", "ready"]) }, legacyHistory: p.legacyHistory,
    accountLabel, modeReason: null, safetyNotice: "Only exact messages from this archive are eligible.", auth: { stage: "ready" }, chats: p.chats.map(chat => discordConversationView(chat, accountLabel)), recentJobs: p.recentJobs.map(jobView) };
}

export function discordSearchFilters(request: SearchRequest): VersionedPayload {
  return { schema: "discord.search_filters", version: 1, payload: { contentKinds: request.contentKinds.map(kind => kind === "photo" ? "image" : kind === "file" ? "document" : kind), direction: request.direction, minDate: request.minDate ?? null, maxDate: request.maxDate ?? null, privacyScan: request.privacyScan ?? false } };
}

export function decodeDiscordSource(value: unknown): DiscordSource { const v = object(value); return { scope: decodeScope(v.scope), accountLabel: text(v.accountLabel), username: optionalNullable(v.username, text), importedAt: optionalNullable(v.importedAt, date), warningCount: count(v.warningCount) }; }
export function decodeDiscordImportProgress(value: unknown): DiscordImportProgress { const v = object(value); return { phase: choice(v.phase, ["inspecting", "hashing", "registering", "importing", "verifying", "ready", "cancelled", "failed"]), inventoryEntries: nullable(v.inventoryEntries, count), processedEntries: count(v.processedEntries), totalBytes: nullable(v.totalBytes, count), hashedBytes: count(v.hashedBytes), readBytes: count(v.readBytes), parsedRecords: count(v.parsedRecords), totalRecords: nullable(v.totalRecords, count), committedItems: count(v.committedItems), committedBytes: count(v.committedBytes), committedBatches: count(v.committedBatches), warnings: count(v.warnings) }; }
export function decodeDiscordImport(value: unknown): DiscordImportStatus { const v = object(value); return { active: bool(v.active), progress: nullable(v.progress, decodeDiscordImportProgress), sources: array(v.sources, decodeDiscordSource), importScope: nullable(v.importScope, decodeScope), retryAvailable: bool(v.retryAvailable), failureCode: nullable(v.failureCode, value => choice(value, ["invalid_archive", "unsupported_profile", "limit_exceeded", "input_changed", "incomplete_source", "storage_failure"])), warningDetails: array(v.warningDetails, value => { const warning = object(value); return { code: text(warning.code), count: count(warning.count) }; }) }; }
export function decodeDiscordSession(value: unknown): DiscordSessionStatus { const v = object(value); return { state: choice(v.state, ["disconnected", "verifying", "ready"]), accountId: optionalNullable(v.accountId, text), username: optionalNullable(v.username, text), displayName: optionalNullable(v.displayName, text), remembered: bool(v.remembered) }; }
export function decodeDiscordBrowser(value: unknown): DiscordBrowser { const v = object(value); return { id: text(v.id), displayName: text(v.displayName), family: choice(v.family, ["chromium_cdp", "firefox_bidi"]) }; }
