import type {
  AppSnapshot,
  AuthSnapshot,
  CatalogProgress,
  ChatSummary,
  ConnectionSettings,
  JobRecord,
  PlanOperation,
  PlanView,
  SearchRequest,
  SearchResponse,
  SaveConnectionSettingsRequest,
  SaveConnectionSettingsResult
} from "./types";

export type AuthCommand =
  | "submit_phone"
  | "submit_email_address"
  | "submit_email_code"
  | "submit_code"
  | "submit_password";

export interface RetractApi {
  isDesktop(): boolean;
  snapshot(): Promise<AppSnapshot>;
  bootstrapSnapshot(): Promise<AppSnapshot>;
  authSnapshot(): Promise<AuthSnapshot>;
  catalogProgress(): Promise<CatalogProgress>;
  connectionSettings(): Promise<ConnectionSettings>;
  saveConnectionSettings(request: SaveConnectionSettingsRequest): Promise<SaveConnectionSettingsResult>;
  search(request: SearchRequest): Promise<SearchResponse>;
  refreshChats(chatIds: number[]): Promise<ChatSummary[]>;
  prepareSelection(messageRefs: Array<{ chatId: number; messageId: number }>): Promise<PlanView>;
  prepareOwnMessages(chatId: number): Promise<PlanView>;
  prepareChatAction(chatId: number, operation: PlanOperation): Promise<PlanView>;
  prepareSenderAction(chatId: number, senderId: number): Promise<PlanView>;
  requestQrAuth(): Promise<void>;
  submitAuth(command: AuthCommand, value: string): Promise<void>;
  execute(plan: PlanView, irreversibleAcknowledged: boolean, typedChatTitle?: string | null): Promise<JobRecord>;
  authorizePlan(plan: PlanView): Promise<void>;
  jobs(): Promise<JobRecord[]>;
  cancelJob(jobId: string): Promise<JobRecord>;
}
