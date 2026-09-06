import { invoke } from "@tauri-apps/api/core";
import type { RetractApi } from "./api-contract";
import { createApi } from "./providers/api";
export const api: RetractApi = createApi((command, request) => invoke<unknown>(command, { request }), () => "__TAURI_INTERNALS__" in window);
