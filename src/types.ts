export type ChatKind =
  | "direct"
  | "basic_group"
  | "supergroup"
  | "channel"
  | "secret";

export type ChatRole =
  | "owner"
  | "admin_with_delete"
  | "admin_limited"
  | "member";

export type ConversationState =
  | "empty"
  | "never_replied"
  | "awaiting_reply"
  | "active"
  | "unknown";

export type DeletionReach = "everyone" | "self_only" | "none";

export type ContentKind =
  | "text"
  | "photo"
  | "video"
  | "file"
  | "voice"
  | "audio"
  | "animation"
  | "sticker"
  | "poll"
  | "location"
  | "contact"
  | "service"
  | "other";

export type SensitiveDataKind =
  | "email_address"
  | "phone_number"
  | "postal_address"
  | "precise_location"
  | "personal_identifier"
  | "identity_document"
  | "financial_account"
  | "crypto_wallet"
  | "credential_or_secret"
  | "network_address"
  | "contact_card";

export interface ChatCapabilities {
  role: ChatRole;
  canDeleteOthers: boolean;
  canClearForEveryone: boolean;
  canRemoveForSelf: boolean;
  canDeleteGroup: boolean;
  canDeleteBySender: boolean;
  canLeaveChat: boolean;
}

export interface ChatSummary {
  id: number;
  title: string;
  kind: ChatKind;
  archived: boolean;
  memberCount?: number | null;
  conversationState: ConversationState;
  capabilities: ChatCapabilities;
  avatarSeed: number;
}

export interface MessageSnapshot {
  chatId: number;
  messageId: number;
  senderId: number;
  senderName: string;
  sentAt: string;
  isOutgoing: boolean;
  contentKind: ContentKind;
  preview: string;
  privacyFindings: SensitiveDataKind[];
  albumId?: number | null;
  isPinned: boolean;
  deletionReach: DeletionReach;
}

export type PlanOperation =
  | "selected_messages"
  | "delete_my_messages"
  | "clear_history"
  | "clear_history_and_leave"
  | "delete_all_messages_and_leave"
  | "remove_chat_for_self"
  | "delete_by_sender"
  | "delete_group"
  | "leave_chat";

export type ConfirmationTier = "low" | "medium" | "high" | "critical";

export interface PlanSummary {
  selected: number;
  deleteForEveryone: number;
  selfOnly: number;
  cannotDelete: number;
}

export interface PlanView {
  id: string;
  operation: PlanOperation;
  chatTitle?: string | null;
  targetSenderName?: string | null;
  summary: PlanSummary;
  confirmationTier: ConfirmationTier;
  fingerprint: string;
  createdAt: string;
}

export type JobStatus =
  | "queued"
  | "running"
  | "completed"
  | "partial"
  | "failed"
  | "cancelled";

export interface JobRecord {
  id: string;
  planId: string;
  operation: PlanOperation;
  targetChatIds: number[];
  status: JobStatus;
  total: number;
  deleted: number;
  skipped: number;
  failed: number;
  nextBatch: number;
  retryAfterSeconds?: number | null;
  errorCodes: string[];
  createdAt: string;
  updatedAt: string;
}

export interface AppSnapshot {
  runtimeMode: "demo" | "live";
  accountLabel: string;
  modeReason?: string | null;
  chats: ChatSummary[];
  recentJobs: JobRecord[];
  safetyNotice: string;
  auth: AuthSnapshot;
}

export interface CatalogProgress {
  phase: "idle" | "discovering" | "loading" | "ready";
  total: number;
  processed: number;
}

export type AuthStage =
  | "initializing"
  | "waiting_for_phone"
  | "waiting_for_email_address"
  | "waiting_for_email_code"
  | "waiting_for_code"
  | "waiting_for_password"
  | "waiting_for_other_device"
  | "ready"
  | "logging_out"
  | "closed"
  | "error";

export interface AuthSnapshot {
  stage: AuthStage;
  hint?: string | null;
  qrLink?: string | null;
}

export type MessageDirection = "any" | "mine" | "others";

export interface SearchRequest {
  query: string;
  chatIds: number[];
  chatKinds: ChatKind[];
  contentKinds: ContentKind[];
  direction: MessageDirection;
  minDate?: string | null;
  maxDate?: string | null;
  excludePinned: boolean;
  privacyScan?: boolean;
  limit: number;
}

export interface SearchResponse {
  messages: MessageSnapshot[];
  returned: number;
  truncated: boolean;
}

export interface CommandError {
  code?: string;
  message?: string;
}

export interface ConnectionSettings {
  setupComplete: boolean;
  tdlibPath: string;
  detectedTdlibPath?: string | null;
  bundledTdlibAvailable: boolean;
  apiId?: number | null;
  apiHashConfigured: boolean;
  useTestDc: boolean;
  environmentOverrides: string[];
  configurationError?: string | null;
  supportedTdlibVersion: string;
}

export interface SaveConnectionSettingsRequest {
  tdlibPath: string;
  apiId: number | null;
  apiHash: string | null;
  useTestDc: boolean;
}

export interface SaveConnectionSettingsResult {
  connectionSettings: ConnectionSettings;
  snapshot: AppSnapshot;
}
