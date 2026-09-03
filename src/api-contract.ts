import type { ActiveContext, ScopedResourceRef, Uuid } from "./providers/identity";
import type { IntentDescriptor } from "./providers/contract";
import type { AppSnapshot, ConnectionSettings, JobRecord, PlanOperation, PlanView, SearchRequest, SearchResponse, SaveConnectionSettingsRequest, SaveConnectionSettingsResult, ChatSummary } from "./types";
export type AuthCommand = "submit_phone" | "submit_email_address" | "submit_email_code" | "submit_code" | "submit_password";
export interface RetractApi {
  isDesktop(): boolean;
  snapshot(context: ActiveContext): Promise<AppSnapshot>;
  bootstrapSnapshot(context?: ActiveContext | null): Promise<AppSnapshot>;
  connectionSettings(context: ActiveContext | null): Promise<ConnectionSettings>;
  saveConnectionSettings(request: SaveConnectionSettingsRequest, context: ActiveContext | null): Promise<SaveConnectionSettingsResult>;
  search(request: SearchRequest, context: ActiveContext): Promise<SearchResponse>;
  refreshChats(conversations: ScopedResourceRef[], context: ActiveContext): Promise<ChatSummary[]>;
  intents(targets: ScopedResourceRef[], context: ActiveContext): Promise<IntentDescriptor[]>;
  prepareSelection(messageRefs: ScopedResourceRef[], context: ActiveContext): Promise<PlanView>;
  prepareOwnMessages(chat: ScopedResourceRef, context: ActiveContext): Promise<PlanView>;
  prepareChatAction(chat: ScopedResourceRef, operation: PlanOperation, context: ActiveContext): Promise<PlanView>;
  prepareSenderAction(chat: ScopedResourceRef, actor: ScopedResourceRef, context: ActiveContext): Promise<PlanView>;
  requestQrAuth(context: ActiveContext | null): Promise<void>;
  submitAuth(command: AuthCommand, value: string, context: ActiveContext | null): Promise<void>;
  retryIdentity(context: ActiveContext | null): Promise<void>;
  execute(plan: PlanView, irreversibleAcknowledged: boolean, typedChatTitle?: string | null): Promise<JobRecord>;
  authorizePlan(plan: PlanView): Promise<void>;
  jobs(context: ActiveContext): Promise<JobRecord[]>;
  cancelJob(jobId: Uuid, context: ActiveContext): Promise<JobRecord>;
}
