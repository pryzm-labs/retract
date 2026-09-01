import { describe, expect, it } from "vitest";
import contract from "./test/fixtures/telegram-ipc-contract.json";
import type {
  AuthSnapshot,
  CatalogProgress,
  JobRecord,
  PlanView,
  SearchRequest,
  SearchResponse,
} from "./types";

function accept<T>(_value: T): void {}

describe("Telegram IPC contract fixture", () => {
  it("uses the field names consumed by the TypeScript boundary", () => {
    accept<SearchRequest>(contract.searchRequest as SearchRequest);
    accept<CatalogProgress>(contract.catalogProgress as CatalogProgress);
    accept<AuthSnapshot>(contract.authSnapshot as AuthSnapshot);
    accept<SearchResponse>(contract.searchResponse as SearchResponse);
    accept<PlanView>(contract.planView as PlanView);
    accept<JobRecord>(contract.jobRecord as JobRecord);

    expect(Object.keys(contract.searchResponse.messages[0]).sort()).toEqual([
      "albumId",
      "chatId",
      "contentKind",
      "deletionReach",
      "isOutgoing",
      "isPinned",
      "messageId",
      "preview",
      "privacyFindings",
      "senderId",
      "senderName",
      "sentAt",
    ]);
    expect(Object.keys(contract.jobRecord).sort()).toEqual([
      "createdAt",
      "deleted",
      "errorCodes",
      "failed",
      "id",
      "nextBatch",
      "operation",
      "planId",
      "retryAfterSeconds",
      "skipped",
      "status",
      "targetChatIds",
      "total",
      "updatedAt",
    ]);
  });

  it("keeps current Telegram identifiers inside the JavaScript safe range", () => {
    const ids = [
      ...contract.searchRequest.chatIds,
      contract.searchResponse.messages[0].chatId,
      contract.searchResponse.messages[0].messageId,
      contract.searchResponse.messages[0].senderId,
      contract.searchResponse.messages[0].albumId,
      ...contract.jobRecord.targetChatIds,
    ];
    expect(ids.every(Number.isSafeInteger)).toBe(true);
  });
});
