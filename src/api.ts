import { invoke } from "@tauri-apps/api/core";
import {
  demoCatalogProgress,
  demoExecute,
  demoJobs,
  demoPrepareChatAction,
  demoPrepareOwnMessages,
  demoPrepareSelection,
  demoPrepareSenderAction,
  demoRefreshChats,
  demoReset,
  demoSearch,
  demoSnapshot
} from "./demo";
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

function runningInTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

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

export const api = {
  isDesktop: runningInTauri,

  snapshot(): Promise<AppSnapshot> {
    return runningInTauri() ? call("get_snapshot") : demoSnapshot();
  },

  bootstrapSnapshot(): Promise<AppSnapshot> {
    return runningInTauri() ? call("get_bootstrap_snapshot") : demoSnapshot();
  },

  authSnapshot(): Promise<AuthSnapshot> {
    return runningInTauri()
      ? call("get_auth_snapshot")
      : demoSnapshot().then((snapshot) => snapshot.auth);
  },

  catalogProgress(): Promise<CatalogProgress> {
    return runningInTauri()
      ? call("get_catalog_progress")
      : demoCatalogProgress();
  },

  connectionSettings(): Promise<ConnectionSettings> {
    return runningInTauri()
      ? call("get_connection_settings")
      : Promise.resolve({
          setupComplete: true,
          tdlibPath: "",
          detectedTdlibPath: null,
          bundledTdlibAvailable: false,
          apiId: null,
          apiHashConfigured: false,
          useTestDc: false,
          environmentOverrides: [],
          configurationError: null,
          supportedTdlibVersion: "1.8.64"
        });
  },

  saveConnectionSettings(request: SaveConnectionSettingsRequest): Promise<SaveConnectionSettingsResult> {
    return runningInTauri()
      ? call("save_connection_settings", { request })
      : Promise.reject(new Error("Connection settings are available in the Retract desktop app."));
  },

  search(request: SearchRequest): Promise<SearchResponse> {
    return runningInTauri() ? call("search_messages", { request }) : demoSearch(request);
  },

  refreshChats(chatIds: number[]): Promise<ChatSummary[]> {
    return runningInTauri()
      ? call("refresh_chats", { chatIds })
      : demoRefreshChats(chatIds);
  },

  prepareSelection(messageRefs: Array<{ chatId: number; messageId: number }>): Promise<PlanView> {
    return runningInTauri()
      ? call("prepare_selection", { request: { messageRefs } })
      : demoPrepareSelection(messageRefs);
  },

  prepareOwnMessages(chatId: number): Promise<PlanView> {
    return runningInTauri()
      ? call("prepare_own_messages", { chatId })
      : demoPrepareOwnMessages(chatId);
  },

  prepareChatAction(chatId: number, operation: PlanOperation): Promise<PlanView> {
    return runningInTauri()
      ? call("prepare_chat_action", { request: { chatId, operation } })
      : demoPrepareChatAction(chatId, operation);
  },

  prepareSenderAction(chatId: number, senderId: number, senderName: string): Promise<PlanView> {
    return runningInTauri()
      ? call("prepare_sender_action", { request: { chatId, senderId, senderName } })
      : demoPrepareSenderAction(chatId, senderId, senderName);
  },

  requestQrAuth(): Promise<void> {
    return call("request_qr_auth");
  },

  submitAuth(command: "submit_phone" | "submit_email_address" | "submit_email_code" | "submit_code" | "submit_password", value: string): Promise<void> {
    return call(command, { request: { value } });
  },

  execute(
    plan: PlanView,
    irreversibleAcknowledged: boolean,
    typedChatTitle?: string | null
  ): Promise<JobRecord> {
    return runningInTauri()
      ? call("start_execution", {
          request: {
            planId: plan.id,
            fingerprint: plan.fingerprint,
            irreversibleAcknowledged,
            typedChatTitle: typedChatTitle || null
          }
        })
      : demoExecute(
          plan.id,
          plan.fingerprint,
          irreversibleAcknowledged,
          typedChatTitle
        );
  },

  authorizePlan(plan: PlanView): Promise<void> {
    return runningInTauri()
      ? call("authorize_plan", {
          request: { planId: plan.id, fingerprint: plan.fingerprint }
        })
      : Promise.resolve();
  },

  jobs(): Promise<JobRecord[]> {
    return runningInTauri() ? call("get_jobs") : demoJobs();
  },

  cancelJob(jobId: string): Promise<JobRecord> {
    return runningInTauri()
      ? call("cancel_job", { jobId })
      : Promise.reject(new Error("Demo cleanup jobs complete immediately."));
  },

  resetDemo(): Promise<AppSnapshot> {
    return runningInTauri() ? call("reset_demo") : demoReset();
  }
};
