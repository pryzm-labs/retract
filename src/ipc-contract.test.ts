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
} from "./test/legacy-types";

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
      (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;
type Assert<Condition extends true> = Condition;

type NormalizeDto<Value> =
  Value extends readonly (infer Item)[]
    ? NormalizeDto<Item>[]
    : Value extends object
      ? { -readonly [Key in keyof Value]: NormalizeDto<Value[Key]> }
      : Value;

type OptionalKeys<Value> = {
  [Key in keyof Value]-?: object extends Pick<Value, Key> ? Key : never;
}[keyof Value];

type RequiredUndefinedKeys<Value> = {
  [Key in keyof Value]-?: object extends Pick<Value, Key>
    ? never
    : undefined extends Value[Key]
      ? Key
      : never;
}[keyof Value];

type NullableFields<Value> = {
  [Key in keyof Value as null extends Value[Key] ? Key : never]: Value[Key];
};

type WireSearchRequest = {
  readonly query: string;
  readonly chatIds: readonly number[];
  readonly chatKinds: readonly ChatKind[];
  readonly contentKinds: readonly ContentKind[];
  readonly direction: MessageDirection;
  readonly minDate?: string | null;
  readonly maxDate?: string | null;
  readonly excludePinned: boolean;
  readonly privacyScan?: boolean;
  readonly limit: number;
};

type WireCatalogProgress = {
  readonly phase: "idle" | "discovering" | "loading" | "ready";
  readonly total: number;
  readonly processed: number;
};

type WireAuthSnapshot = {
  readonly stage: AuthStage;
  readonly hint?: string | null;
  readonly qrLink?: string | null;
};

type WireMessageSnapshot = {
  readonly chatId: number;
  readonly messageId: number;
  readonly senderId: number;
  readonly senderName: string;
  readonly sentAt: string;
  readonly isOutgoing: boolean;
  readonly contentKind: ContentKind;
  readonly preview: string;
  readonly privacyFindings: readonly SensitiveDataKind[];
  readonly albumId?: number | null;
  readonly isPinned: boolean;
  readonly deletionReach: DeletionReach;
};

type WireSearchResponse = {
  readonly messages: readonly WireMessageSnapshot[];
  readonly returned: number;
  readonly truncated: boolean;
};

type WirePlanSummary = {
  readonly selected: number;
  readonly deleteForEveryone: number;
  readonly selfOnly: number;
  readonly cannotDelete: number;
};

type WirePlanView = {
  readonly id: string;
  readonly operation: PlanOperation;
  readonly chatTitle?: string | null;
  readonly targetSenderName?: string | null;
  readonly summary: WirePlanSummary;
  readonly confirmationTier: ConfirmationTier;
  readonly fingerprint: string;
  readonly createdAt: string;
};

type WireJobRecord = {
  readonly id: string;
  readonly planId: string;
  readonly operation: PlanOperation;
  readonly targetChatIds: readonly number[];
  readonly status: JobStatus;
  readonly total: number;
  readonly deleted: number;
  readonly skipped: number;
  readonly failed: number;
  readonly nextBatch: number;
  readonly retryAfterSeconds?: number | null;
  readonly errorCodes: readonly string[];
  readonly createdAt: string;
  readonly updatedAt: string;
};

type WireContract = {
  readonly searchRequest: WireSearchRequest;
  readonly catalogProgress: WireCatalogProgress;
  readonly authSnapshot: WireAuthSnapshot;
  readonly searchResponse: WireSearchResponse;
  readonly planView: WirePlanView;
  readonly jobRecord: WireJobRecord;
};

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
} satisfies WireContract;

type _SearchRequestDto = Assert<Equal<NormalizeDto<WireSearchRequest>, SearchRequest>>;
type _CatalogProgressDto = Assert<Equal<NormalizeDto<WireCatalogProgress>, CatalogProgress>>;
type _AuthSnapshotDto = Assert<Equal<NormalizeDto<WireAuthSnapshot>, AuthSnapshot>>;
type _SearchResponseDto = Assert<Equal<NormalizeDto<WireSearchResponse>, SearchResponse>>;
type _MessageSnapshotDto = Assert<Equal<NormalizeDto<WireMessageSnapshot>, MessageSnapshot>>;
type _PlanViewDto = Assert<Equal<NormalizeDto<WirePlanView>, PlanView>>;
type _PlanSummaryDto = Assert<Equal<NormalizeDto<WirePlanSummary>, PlanSummary>>;
type _JobRecordDto = Assert<Equal<NormalizeDto<WireJobRecord>, JobRecord>>;

type _SearchRequestOptionalKeys = Assert<
  Equal<OptionalKeys<WireSearchRequest>, OptionalKeys<SearchRequest>>
>;
type _CatalogProgressOptionalKeys = Assert<
  Equal<OptionalKeys<WireCatalogProgress>, OptionalKeys<CatalogProgress>>
>;
type _AuthSnapshotOptionalKeys = Assert<
  Equal<OptionalKeys<WireAuthSnapshot>, OptionalKeys<AuthSnapshot>>
>;
type _SearchResponseOptionalKeys = Assert<
  Equal<OptionalKeys<WireSearchResponse>, OptionalKeys<SearchResponse>>
>;
type _MessageSnapshotOptionalKeys = Assert<
  Equal<OptionalKeys<WireMessageSnapshot>, OptionalKeys<MessageSnapshot>>
>;
type _PlanViewOptionalKeys = Assert<
  Equal<OptionalKeys<WirePlanView>, OptionalKeys<PlanView>>
>;
type _PlanSummaryOptionalKeys = Assert<
  Equal<OptionalKeys<WirePlanSummary>, OptionalKeys<PlanSummary>>
>;
type _JobRecordOptionalKeys = Assert<
  Equal<OptionalKeys<WireJobRecord>, OptionalKeys<JobRecord>>
>;

type _SearchRequestRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireSearchRequest>, RequiredUndefinedKeys<SearchRequest>>
>;
type _CatalogProgressRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireCatalogProgress>, RequiredUndefinedKeys<CatalogProgress>>
>;
type _AuthSnapshotRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireAuthSnapshot>, RequiredUndefinedKeys<AuthSnapshot>>
>;
type _SearchResponseRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireSearchResponse>, RequiredUndefinedKeys<SearchResponse>>
>;
type _MessageSnapshotRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireMessageSnapshot>, RequiredUndefinedKeys<MessageSnapshot>>
>;
type _PlanViewRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WirePlanView>, RequiredUndefinedKeys<PlanView>>
>;
type _PlanSummaryRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WirePlanSummary>, RequiredUndefinedKeys<PlanSummary>>
>;
type _JobRecordRequiredUndefined = Assert<
  Equal<RequiredUndefinedKeys<WireJobRecord>, RequiredUndefinedKeys<JobRecord>>
>;

type _SearchRequestNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireSearchRequest>>, NullableFields<SearchRequest>>
>;
type _CatalogProgressNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireCatalogProgress>>, NullableFields<CatalogProgress>>
>;
type _AuthSnapshotNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireAuthSnapshot>>, NullableFields<AuthSnapshot>>
>;
type _SearchResponseNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireSearchResponse>>, NullableFields<SearchResponse>>
>;
type _MessageSnapshotNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireMessageSnapshot>>, NullableFields<MessageSnapshot>>
>;
type _PlanViewNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WirePlanView>>, NullableFields<PlanView>>
>;
type _PlanSummaryNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WirePlanSummary>>, NullableFields<PlanSummary>>
>;
type _JobRecordNullableFields = Assert<
  Equal<NormalizeDto<NullableFields<WireJobRecord>>, NullableFields<JobRecord>>
>;

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
