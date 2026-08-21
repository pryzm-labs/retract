import type { RetractApi } from "./api-contract";
import type { ConnectionSettings } from "./types";
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

const connectionDefaults: ConnectionSettings = {
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
};

export const api: RetractApi = {
  isDesktop: () => false,
  snapshot: demoSnapshot,
  bootstrapSnapshot: demoSnapshot,
  authSnapshot: () => demoSnapshot().then((snapshot) => snapshot.auth),
  catalogProgress: demoCatalogProgress,
  connectionSettings: () => Promise.resolve(connectionDefaults),
  saveConnectionSettings: () => Promise.reject(new Error("Connection settings require the Retract desktop app.")),
  search: demoSearch,
  refreshChats: demoRefreshChats,
  prepareSelection: demoPrepareSelection,
  prepareOwnMessages: demoPrepareOwnMessages,
  prepareChatAction: demoPrepareChatAction,
  prepareSenderAction: demoPrepareSenderAction,
  requestQrAuth: () => Promise.reject(new Error("Telegram authentication is unavailable in fixture mode.")),
  submitAuth: () => Promise.reject(new Error("Telegram authentication is unavailable in fixture mode.")),
  execute: (plan, irreversibleAcknowledged, typedChatTitle) =>
    demoExecute(plan.id, plan.fingerprint, irreversibleAcknowledged, typedChatTitle),
  authorizePlan: () => Promise.resolve(),
  jobs: demoJobs,
  cancelJob: () => Promise.reject(new Error("Fixture cleanup jobs complete immediately."))
};

export const fixtureApi = {
  resetFixtures: demoReset
};
