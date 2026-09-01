import { describe, expect, it } from "vitest";
import contract from "./test/fixtures/telegram-ipc-contract.json";
import type {
  AuthSnapshot,
  AuthStage,
  CatalogProgress,
  ChatKind,
  ChatRole,
  ConfirmationTier,
  ContentKind,
  ConversationState,
  DeletionReach,
  JobRecord,
  JobStatus,
  MessageDirection,
  MessageSnapshot,
  PlanOperation,
  PlanSummary,
  PlanView,
  SearchRequest,
  SearchResponse,
  SensitiveDataKind,
} from "./types";

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
      (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;
type Assert<Condition extends true> = Condition;

const typedContract = {
  searchRequest: {
    query: "passport apartment",
    chatIds: [-1001],
    chatKinds: ["supergroup"],
    contentKinds: ["photo", "file"],
    direction: "mine",
    minDate: "2026-08-01T00:00:00Z",
    maxDate: "2026-08-31T23:59:59Z",
    excludePinned: true,
    privacyScan: true,
    limit: 500,
  },
  catalogProgress: { phase: "loading", total: 531, processed: 128 },
  authSnapshot: {
    stage: "waiting_for_password",
    hint: "synthetic hint",
    qrLink: null,
  },
  searchResponse: {
    messages: [
      {
        chatId: -1001,
        messageId: 14,
        senderId: 42,
        senderName: "Synthetic user",
        sentAt: "2026-08-15T18:00:00Z",
        isOutgoing: true,
        contentKind: "photo",
        preview: "Synthetic passport reminder",
        privacyFindings: ["identity_document"],
        albumId: 7001,
        isPinned: false,
        deletionReach: "everyone",
      },
    ],
    returned: 1,
    truncated: false,
  },
  planView: {
    id: "11111111-1111-4111-8111-111111111111",
    operation: "selected_messages",
    chatTitle: null,
    targetSenderName: null,
    summary: {
      selected: 1,
      deleteForEveryone: 1,
      selfOnly: 0,
      cannotDelete: 0,
    },
    confirmationTier: "low",
    fingerprint: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    createdAt: "2026-08-15T18:01:00Z",
  },
  jobRecord: {
    id: "22222222-2222-4222-8222-222222222222",
    planId: "11111111-1111-4111-8111-111111111111",
    operation: "selected_messages",
    targetChatIds: [-1001],
    status: "completed",
    total: 1,
    deleted: 1,
    skipped: 0,
    failed: 0,
    nextBatch: 1,
    retryAfterSeconds: null,
    errorCodes: [],
    createdAt: "2026-08-15T18:01:01Z",
    updatedAt: "2026-08-15T18:01:02Z",
  },
} satisfies {
  searchRequest: SearchRequest;
  catalogProgress: CatalogProgress;
  authSnapshot: AuthSnapshot;
  searchResponse: SearchResponse;
  planView: PlanView;
  jobRecord: JobRecord;
};

type _SearchRequestKeys = Assert<Equal<keyof typeof typedContract.searchRequest, keyof SearchRequest>>;
type _CatalogProgressKeys = Assert<Equal<keyof typeof typedContract.catalogProgress, keyof CatalogProgress>>;
type _AuthSnapshotKeys = Assert<Equal<keyof typeof typedContract.authSnapshot, keyof AuthSnapshot>>;
type _SearchResponseKeys = Assert<Equal<keyof typeof typedContract.searchResponse, keyof SearchResponse>>;
type _MessageSnapshotKeys = Assert<
  Equal<keyof (typeof typedContract.searchResponse.messages)[number], keyof MessageSnapshot>
>;
type _PlanViewKeys = Assert<Equal<keyof typeof typedContract.planView, keyof PlanView>>;
type _PlanSummaryKeys = Assert<Equal<keyof typeof typedContract.planView.summary, keyof PlanSummary>>;
type _JobRecordKeys = Assert<Equal<keyof typeof typedContract.jobRecord, keyof JobRecord>>;

const WIRE_ENUMS = {
  authStage: [
    "initializing", "waiting_for_phone", "waiting_for_email_address",
    "waiting_for_email_code", "waiting_for_code", "waiting_for_password",
    "waiting_for_other_device", "ready", "logging_out", "closed", "error",
  ],
  catalogPhase: ["idle", "discovering", "loading", "ready"],
  chatKind: ["direct", "basic_group", "supergroup", "channel", "secret"],
  chatRole: ["owner", "admin_with_delete", "admin_limited", "member"],
  conversationState: ["empty", "never_replied", "awaiting_reply", "active", "unknown"],
  deletionReach: ["everyone", "self_only", "none"],
  contentKind: [
    "text", "photo", "video", "file", "voice", "audio", "animation",
    "sticker", "poll", "location", "contact", "service", "other",
  ],
  sensitiveDataKind: [
    "email_address", "phone_number", "postal_address", "precise_location",
    "personal_identifier", "identity_document", "financial_account",
    "crypto_wallet", "credential_or_secret", "network_address", "contact_card",
  ],
  planOperation: [
    "selected_messages", "delete_my_messages", "clear_history",
    "clear_history_and_leave", "delete_all_messages_and_leave",
    "remove_chat_for_self", "delete_by_sender", "delete_group", "leave_chat",
  ],
  confirmationTier: ["low", "medium", "high", "critical"],
  jobStatus: ["queued", "running", "completed", "partial", "failed", "cancelled"],
  messageDirection: ["any", "mine", "others"],
} as const;

type _AllAuthStages = Assert<Equal<(typeof WIRE_ENUMS.authStage)[number], AuthStage>>;
type _AllCatalogPhases = Assert<
  Equal<(typeof WIRE_ENUMS.catalogPhase)[number], CatalogProgress["phase"]>
>;
type _AllChatKinds = Assert<Equal<(typeof WIRE_ENUMS.chatKind)[number], ChatKind>>;
type _AllChatRoles = Assert<Equal<(typeof WIRE_ENUMS.chatRole)[number], ChatRole>>;
type _AllConversationStates = Assert<
  Equal<(typeof WIRE_ENUMS.conversationState)[number], ConversationState>
>;
type _AllDeletionReaches = Assert<Equal<(typeof WIRE_ENUMS.deletionReach)[number], DeletionReach>>;
type _AllContentKinds = Assert<Equal<(typeof WIRE_ENUMS.contentKind)[number], ContentKind>>;
type _AllSensitiveDataKinds = Assert<
  Equal<(typeof WIRE_ENUMS.sensitiveDataKind)[number], SensitiveDataKind>
>;
type _AllPlanOperations = Assert<Equal<(typeof WIRE_ENUMS.planOperation)[number], PlanOperation>>;
type _AllConfirmationTiers = Assert<
  Equal<(typeof WIRE_ENUMS.confirmationTier)[number], ConfirmationTier>
>;
type _AllJobStatuses = Assert<Equal<(typeof WIRE_ENUMS.jobStatus)[number], JobStatus>>;
type _AllMessageDirections = Assert<
  Equal<(typeof WIRE_ENUMS.messageDirection)[number], MessageDirection>
>;

const nullableBoundaries = {
  search: { minDate: null, maxDate: null, privacyScan: undefined },
  auth: { hint: null, qrLink: null },
  message: { albumId: null },
  plan: { chatTitle: null, targetSenderName: null },
  job: { retryAfterSeconds: null },
} satisfies {
  search: Pick<SearchRequest, "minDate" | "maxDate" | "privacyScan">;
  auth: Pick<AuthSnapshot, "hint" | "qrLink">;
  message: Pick<MessageSnapshot, "albumId">;
  plan: Pick<PlanView, "chatTitle" | "targetSenderName">;
  job: Pick<JobRecord, "retryAfterSeconds">;
};

function expectExactKeys(value: object, expected: readonly string[]): void {
  expect(Object.keys(value).sort()).toEqual([...expected].sort());
}

describe("Telegram IPC contract fixture", () => {
  it("matches cast-free TypeScript DTOs and exact runtime key sets", () => {
    expect(contract).toEqual(typedContract);
    expectExactKeys(contract, [
      "authSnapshot", "catalogProgress", "jobRecord", "planView",
      "searchRequest", "searchResponse",
    ]);
    expectExactKeys(contract.searchRequest, [
      "query", "chatIds", "chatKinds", "contentKinds", "direction", "minDate",
      "maxDate", "excludePinned", "privacyScan", "limit",
    ]);
    expectExactKeys(contract.catalogProgress, ["phase", "total", "processed"]);
    expectExactKeys(contract.authSnapshot, ["stage", "hint", "qrLink"]);
    expectExactKeys(contract.searchResponse, ["messages", "returned", "truncated"]);
    expectExactKeys(contract.searchResponse.messages[0], [
      "chatId", "messageId", "senderId", "senderName", "sentAt", "isOutgoing",
      "contentKind", "preview", "privacyFindings", "albumId", "isPinned",
      "deletionReach",
    ]);
    expectExactKeys(contract.planView, [
      "id", "operation", "chatTitle", "targetSenderName", "summary",
      "confirmationTier", "fingerprint", "createdAt",
    ]);
    expectExactKeys(contract.planView.summary, [
      "selected", "deleteForEveryone", "selfOnly", "cannotDelete",
    ]);
    expectExactKeys(contract.jobRecord, [
      "id", "planId", "operation", "targetChatIds", "status", "total", "deleted",
      "skipped", "failed", "nextBatch", "retryAfterSeconds", "errorCodes",
      "createdAt", "updatedAt",
    ]);
  });

  it("enumerates every wire enum and nullable or optional boundary", () => {
    expect(WIRE_ENUMS.authStage).toContain(contract.authSnapshot.stage);
    expect(WIRE_ENUMS.catalogPhase).toContain(contract.catalogProgress.phase);
    expect(WIRE_ENUMS.chatKind).toContain(contract.searchRequest.chatKinds[0]);
    expect(WIRE_ENUMS.contentKind).toContain(contract.searchResponse.messages[0].contentKind);
    expect(WIRE_ENUMS.deletionReach).toContain(contract.searchResponse.messages[0].deletionReach);
    expect(WIRE_ENUMS.sensitiveDataKind).toContain(
      contract.searchResponse.messages[0].privacyFindings[0],
    );
    expect(WIRE_ENUMS.planOperation).toContain(contract.planView.operation);
    expect(WIRE_ENUMS.confirmationTier).toContain(contract.planView.confirmationTier);
    expect(WIRE_ENUMS.jobStatus).toContain(contract.jobRecord.status);
    expect(WIRE_ENUMS.messageDirection).toContain(contract.searchRequest.direction);
    expect(contract.authSnapshot.qrLink).toBeNull();
    expect(contract.planView.chatTitle).toBeNull();
    expect(contract.planView.targetSenderName).toBeNull();
    expect(contract.jobRecord.retryAfterSeconds).toBeNull();
    expect(nullableBoundaries).toEqual({
      search: { minDate: null, maxDate: null, privacyScan: undefined },
      auth: { hint: null, qrLink: null },
      message: { albumId: null },
      plan: { chatTitle: null, targetSenderName: null },
      job: { retryAfterSeconds: null },
    });
  });

  it("keeps Phase 0 Telegram identifiers inside the JavaScript safe range", () => {
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
