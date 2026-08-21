import { invoke } from "@tauri-apps/api/core";
import type { AuthCommand, RetractApi } from "./api-contract";
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

function normalizeError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (typeof error === "string") return new Error(error);
  if (error && typeof error === "object" && "message" in error) {
    return new Error(String((error as { message: unknown }).message));
  }
  return new Error("An unexpected local error occurred.");
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw normalizeError(error);
  }
}

export const api: RetractApi = {
  isDesktop: () => "__TAURI_INTERNALS__" in window,
  snapshot: () => call<AppSnapshot>("get_snapshot"),
  bootstrapSnapshot: () => call<AppSnapshot>("get_bootstrap_snapshot"),
  authSnapshot: () => call<AuthSnapshot>("get_auth_snapshot"),
  catalogProgress: () => call<CatalogProgress>("get_catalog_progress"),
  connectionSettings: () => call<ConnectionSettings>("get_connection_settings"),
  saveConnectionSettings: (request: SaveConnectionSettingsRequest) =>
    call<SaveConnectionSettingsResult>("save_connection_settings", { request }),
  search: (request: SearchRequest) => call<SearchResponse>("search_messages", { request }),
  refreshChats: (chatIds: number[]) => call<ChatSummary[]>("refresh_chats", { chatIds }),
  prepareSelection: (messageRefs: Array<{ chatId: number; messageId: number }>) =>
    call<PlanView>("prepare_selection", { request: { messageRefs } }),
  prepareOwnMessages: (chatId: number) => call<PlanView>("prepare_own_messages", { chatId }),
  prepareChatAction: (chatId: number, operation: PlanOperation) =>
    call<PlanView>("prepare_chat_action", { request: { chatId, operation } }),
  prepareSenderAction: (chatId: number, senderId: number, senderName: string) =>
    call<PlanView>("prepare_sender_action", { request: { chatId, senderId, senderName } }),
  requestQrAuth: () => call<void>("request_qr_auth"),
  submitAuth: (command: AuthCommand, value: string) => call<void>(command, { request: { value } }),
  execute: (plan: PlanView, irreversibleAcknowledged: boolean, typedChatTitle?: string | null) =>
    call<JobRecord>("start_execution", {
      request: {
        planId: plan.id,
        fingerprint: plan.fingerprint,
        irreversibleAcknowledged,
        typedChatTitle: typedChatTitle || null
      }
    }),
  authorizePlan: (plan: PlanView) =>
    call<void>("authorize_plan", { request: { planId: plan.id, fingerprint: plan.fingerprint } }),
  jobs: () => call<JobRecord[]>("get_jobs"),
  cancelJob: (jobId: string) => call<JobRecord>("cancel_job", { jobId })
};
