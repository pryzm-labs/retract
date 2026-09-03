// Build-time screenshot/test transport only; production resolves api.desktop.ts.
import { createApi } from "./providers/api";
import { array, bool, choice, count, date, decodeContext, decodeRefs, invalid, nullable, object, text, type ConversationRecord, type ContentRecord } from "./providers/contract";
import { sameContext, uuid } from "./providers/identity";
import type { ChatSummary, MessageSnapshot } from "./types";
import { demoExecute, demoIntents, demoJobs, demoPrepareChatAction, demoPrepareOwnMessages, demoPrepareSelection, demoPrepareSenderAction, demoRefreshChats, demoReset, demoSearch, demoSnapshot, fixtureContext, fixtureRef } from "./demo";
const connectionDefaults = { setupComplete: true, tdlibPath: "", detectedTdlibPath: null, bundledTdlibAvailable: false, apiId: null, apiHashConfigured: false, useTestDc: false, environmentOverrides: [], configurationError: null, supportedTdlibVersion: "1.8.64" };
export function fixtureConversation(chat: ChatSummary): ConversationRecord {
  return { id: chat.id, scope: chat.scope, resource: chat.ref.resource, kind: chat.kind === "channel" ? "broadcast" : chat.kind === "supergroup" || chat.kind === "basic_group" ? "group" : "direct", title: chat.title, parentId: null, participantCount: chat.memberCount ?? null, participants: [], evidence: "live", observedAt: "2026-08-15T18:00:00Z", providerMetadata: { schema: "telegram.conversation_metadata", version: 1, payload: { originalKind: chat.kind, archived: chat.archived, conversationState: chat.conversationState, capabilities: chat.capabilities, avatarSeed: chat.avatarSeed } } };
}
export function fixtureContent(message: MessageSnapshot): ContentRecord {
  // The stable grouping ref is retained by lookup from fixture identity, never by parsing a locator.
  const grouping = message.albumId ? { scope: message.scope, id: message.albumId, resource: fixtureRef("grouping", "-1001", "7001").resource } : null;
  return { id: message.messageId, scope: message.scope, conversationId: message.chatId, resource: message.ref.resource, authorId: message.senderId, timestamp: message.sentAt, editedAt: null, kind: message.contentKind === "photo" ? "image" : message.contentKind === "file" ? "document" : message.contentKind, searchableText: message.preview, attachments: [], replyTo: null, threadParent: null, externalLocation: "unsupported", evidence: "live", observedAt: message.sentAt, privacyFindings: message.privacyFindings, detectorVersion: message.privacyFindings.length ? "fixture-sensitive-v1" : null, providerMetadata: { schema: "telegram.content_metadata", version: 1, payload: { originalKind: message.contentKind, outgoing: message.isOutgoing, pinned: message.isPinned, grouping, actor: message.actorRef, senderName: message.senderName, deletionReach: message.deletionReach } } };
}
export const api = createApi(async (command, raw) => {
  const request = object(raw);
  if (request.contractVersion !== 2) return invalid();
  if (command !== "get_bootstrap_snapshot_v2" || request.context !== null) {
    if (!sameContext(decodeContext(request.context), fixtureContext)) return invalid();
  }
  const p = object(request.payload);
  let payload: unknown;
  switch (command) {
    case "get_bootstrap_snapshot_v2":
    case "get_snapshot_v2": { const s = await demoSnapshot(); payload = { identity: s.identity, auth: { schema: "telegram.auth", version: 1, payload: s.auth }, catalog: s.catalog, chats: s.chats.map(fixtureConversation), recentJobs: s.recentJobs, legacyHistory: [] }; break; }
    case "get_connection_settings_v2": payload = connectionDefaults; break;
    case "search_messages_v2": {
      const filters = object(p.filters), f = object(filters.payload);
      if (filters.schema !== "telegram.search_filters" || filters.version !== 1) return invalid();
      const response = await demoSearch({ query: text(p.query), conversations: decodeRefs(p.conversations, fixtureContext.scope, "conversation"), limit: count(p.limit, 10_000),
        chatKinds: array(f.chatKinds, v => choice(v, ["direct", "basic_group", "supergroup", "channel", "secret"])),
        contentKinds: array(f.contentKinds, v => choice(v, ["text", "photo", "video", "file", "voice", "audio", "animation", "sticker", "poll", "location", "contact", "service", "other"])),
        direction: choice(f.direction, ["any", "mine", "others"]), minDate: nullable(f.minDate, date), maxDate: nullable(f.maxDate, date), excludePinned: bool(f.excludePinned), privacyScan: bool(f.privacyScan) });
      payload = { items: response.messages.map(fixtureContent), nextCursor: response.truncated ? "more" : null }; break;
    }
    case "refresh_chats_v2": payload = (await demoRefreshChats(decodeRefs(p.conversations, fixtureContext.scope, "conversation").map(ref => uuid<"ConversationId">(ref.id)))).map(fixtureConversation); break;
    case "get_intents_v2": payload = await demoIntents(decodeRefs(p.targets, fixtureContext.scope)); break;
    case "prepare_selection_v2": payload = await demoPrepareSelection(decodeRefs(p.messageRefs, fixtureContext.scope, "content")); break;
    case "prepare_intent_v2": {
      const refs = decodeRefs(p.targets, fixtureContext.scope, "conversation"), operation = choice(p.actionId, ["selected_messages", "delete_my_messages", "clear_history", "clear_history_and_leave", "delete_all_messages_and_leave", "remove_chat_for_self", "delete_by_sender", "delete_group", "leave_chat"]);
      if (refs.length !== 1) return invalid();
      const chatId = uuid<"ConversationId">(refs[0].id);
      payload = operation === "delete_my_messages" ? await demoPrepareOwnMessages(chatId) : operation === "delete_by_sender" ? await demoPrepareSenderAction(chatId, uuid<"ActorId">(decodeRefs([p.actor], fixtureContext.scope, "actor")[0].id)) : await demoPrepareChatAction(chatId, operation); break;
    }
    case "authorize_plan_v2": payload = {}; break;
    case "start_execution_v2": payload = await demoExecute(uuid(p.planId), text(p.fingerprint), bool(p.irreversibleAcknowledged), nullable(p.typedChatTitle, text)); break;
    case "get_jobs_v2": payload = await demoJobs(); break;
    default: throw new Error("This operation requires the Retract desktop app.");
  }
  return { contractVersion: 2, context: fixtureContext, payload };
}, () => false);
export const fixtureApi = { resetFixtures: demoReset };
