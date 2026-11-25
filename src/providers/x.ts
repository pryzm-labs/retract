import type { AppSnapshot, MessageSnapshot } from "../types";
import { choice, invalid, object, recordRef, text, type BootstrapResponse, type BootstrapSnapshot, type ContentRecord } from "./contract";

export function xSnapshotView(response: BootstrapResponse<BootstrapSnapshot>): AppSnapshot {
  const { auth, catalog, identity } = response.payload;
  if (auth?.schema !== "x.account" || auth.version !== 1 || response.payload.recentJobs.length) return invalid();
  return {
    context: response.context, identity, auth: { stage: "ready" },
    accountLabel: text(object(auth.payload).accountLabel),
    catalog: { ...catalog, phase: choice(catalog.phase, ["ready"]) },
    chats: [], recentJobs: [], legacyHistory: [], safetyNotice: "Offline archive search"
  };
}

export function xContentView(record: ContentRecord): MessageSnapshot {
  if (record.evidence !== "archive" || record.kind !== "text") return invalid();
  return {
    scope: record.scope, ref: recordRef(record), actorRef: null, chatId: record.conversationId,
    messageId: record.id, senderId: record.authorId, senderName: "You", sentAt: record.timestamp,
    isOutgoing: true, contentKind: "text", preview: record.searchableText,
    privacyFindings: record.privacyFindings, isPinned: false, deletionReach: "none"
  };
}
