import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CustomRoutingPreset, CustomTaskRoute } from "./customAiRoutes";

const BROWSER_SESSION_KEY = "mpgs.ai.custom.session.v2";
const LEGACY_BROWSER_KEY = "mpgs.ai.custom.session.v1";

export interface LocalCustomAiSettings {
  userId: string;
  baseUrl: string;
  /** Default / construction-time model (also primary for single mode). */
  model: string;
  apiKey: string;
  /** easy = multi-model auto plan; single = one model for all tasks. */
  routingPreset: CustomRoutingPreset;
  fallbackModel?: string | null;
  routes?: CustomTaskRoute[];
}

function parse(value: string | null, userId: string): LocalCustomAiSettings | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value) as Partial<LocalCustomAiSettings> & {
      // v1 fields only had model/baseUrl/apiKey/userId
    };
    if (
      parsed.userId !== userId ||
      typeof parsed.baseUrl !== "string" ||
      typeof parsed.model !== "string" ||
      typeof parsed.apiKey !== "string" ||
      !parsed.apiKey
    ) {
      return null;
    }
    const routingPreset: CustomRoutingPreset =
      parsed.routingPreset === "single" || parsed.routingPreset === "easy"
        ? parsed.routingPreset
        : "easy";
    return {
      userId,
      baseUrl: parsed.baseUrl,
      model: parsed.model,
      apiKey: parsed.apiKey,
      routingPreset,
      fallbackModel: parsed.fallbackModel ?? null,
      routes: Array.isArray(parsed.routes) ? parsed.routes : undefined,
    };
  } catch {
    return null;
  }
}

export async function loadLocalCustomAiSettings(
  userId: string,
): Promise<LocalCustomAiSettings | null> {
  if (isTauri()) {
    const raw = await invoke<string | null>("ai_credential_load");
    return parse(raw, userId);
  }
  const modern = parse(globalThis.sessionStorage.getItem(BROWSER_SESSION_KEY), userId);
  if (modern) return modern;
  // Migrate v1 session key once.
  const legacy = parse(globalThis.sessionStorage.getItem(LEGACY_BROWSER_KEY), userId);
  if (legacy) {
    await saveLocalCustomAiSettings(legacy);
    globalThis.sessionStorage.removeItem(LEGACY_BROWSER_KEY);
  }
  return legacy;
}

export async function saveLocalCustomAiSettings(
  settings: LocalCustomAiSettings,
): Promise<void> {
  const value = JSON.stringify(settings);
  if (isTauri()) {
    await invoke("ai_credential_save", { value });
  } else {
    globalThis.sessionStorage.setItem(BROWSER_SESSION_KEY, value);
  }
}

export async function removeLocalCustomAiSettings(): Promise<void> {
  if (isTauri()) {
    await invoke("ai_credential_remove");
  } else {
    globalThis.sessionStorage.removeItem(BROWSER_SESSION_KEY);
    globalThis.sessionStorage.removeItem(LEGACY_BROWSER_KEY);
  }
}
