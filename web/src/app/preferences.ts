// Preference form helpers: option lists, defaults, and change detection so the
// settings screen only PUTs when something actually differs.

import type { StorageLike, UserPreferences } from "../api/types";
import { getServiceStorage } from "../api/storage";

const PENDING_PREFERENCES_KEY = "mpgs.preferences.pending.v1";

export type PendingPreferencesPatch = Omit<Partial<UserPreferences>, "version">;

interface PreferencesApi {
  getPreferences(): Promise<UserPreferences>;
  putPreferences(preferences: UserPreferences): Promise<UserPreferences>;
}

export const PLATFORM_OPTIONS: { id: string; label: string }[] = [
  { id: "windows", label: "Windows" },
  { id: "macos", label: "macOS" },
  { id: "linux", label: "Linux" },
];

export const LANGUAGE_OPTIONS: { id: string; label: string }[] = [
  { id: "schinese", label: "简体中文" },
  { id: "tchinese", label: "繁体中文" },
  { id: "english", label: "英语" },
  { id: "japanese", label: "日语" },
  { id: "koreana", label: "韩语" },
];

export const EXCLUDED_MODE_OPTIONS: { id: string; label: string }[] = [
  { id: "mmo", label: "MMO" },
  { id: "battle_royale", label: "大逃杀" },
  { id: "pvp_only", label: "纯 PvP" },
];

export const SESSION_OPTIONS: { label: string; min: number; max: number }[] = [
  { label: "30–60 分钟", min: 30, max: 60 },
  { label: "1–2 小时", min: 60, max: 120 },
  { label: "2–3 小时", min: 120, max: 180 },
  { label: "不设限", min: 15, max: 480 },
];

export function defaultPreferences(): UserPreferences {
  return {
    version: 1,
    party_size: 4,
    coop_competitive: 0.15,
    session_minutes_min: 30,
    session_minutes_max: 180,
    budget_currency: "CNY",
    budget_max_each_minor: 15000,
    platforms: ["windows"],
    self_hosting_willingness: 0.7,
    languages: ["schinese", "english"],
    excluded_modes: ["mmo"],
  };
}

function sameSet(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const set = new Set(a);
  return b.every((item) => set.has(item));
}

/** True when the editable fields of `next` differ from `base` (version ignored). */
export function preferencesChanged(base: UserPreferences, next: UserPreferences): boolean {
  return (
    base.party_size !== next.party_size ||
    base.coop_competitive !== next.coop_competitive ||
    base.session_minutes_min !== next.session_minutes_min ||
    base.session_minutes_max !== next.session_minutes_max ||
    base.budget_currency !== next.budget_currency ||
    base.budget_max_each_minor !== next.budget_max_each_minor ||
    base.self_hosting_willingness !== next.self_hosting_willingness ||
    !sameSet(base.platforms, next.platforms) ||
    !sameSet(base.languages, next.languages) ||
    !sameSet(base.excluded_modes, next.excluded_modes)
  );
}

export function editablePreferencePatch(preferences: UserPreferences): PendingPreferencesPatch {
  return {
    party_size: preferences.party_size,
    coop_competitive: preferences.coop_competitive,
    session_minutes_min: preferences.session_minutes_min,
    session_minutes_max: preferences.session_minutes_max,
    budget_currency: preferences.budget_currency,
    budget_max_each_minor: preferences.budget_max_each_minor,
    platforms: preferences.platforms,
    self_hosting_willingness: preferences.self_hosting_willingness,
    languages: preferences.languages,
    excluded_modes: preferences.excluded_modes,
  };
}

/** Toggle membership of `id` in `list`, returning a new array. */
export function toggleMember(list: string[], id: string): string[] {
  return list.includes(id) ? list.filter((item) => item !== id) : [...list, id];
}

/** Persist a preference edit before attempting network I/O. */
export function queuePreferencePatch(
  patch: PendingPreferencesPatch,
  storage: StorageLike = getServiceStorage(),
): boolean {
  try {
    storage.setItem(PENDING_PREFERENCES_KEY, JSON.stringify(patch));
    return true;
  } catch {
    return false;
  }
}

export function hasPendingPreferencePatch(
  storage: StorageLike = getServiceStorage(),
): boolean {
  try {
    return storage.getItem(PENDING_PREFERENCES_KEY) !== null;
  } catch {
    return false;
  }
}

function loadPendingPreferencePatch(storage: StorageLike): PendingPreferencesPatch | null {
  try {
    const raw = storage.getItem(PENDING_PREFERENCES_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return parsed !== null && typeof parsed === "object"
      ? (parsed as PendingPreferencesPatch)
      : null;
  } catch {
    return null;
  }
}

export function applyPendingPreferencePatch(
  preferences: UserPreferences,
  storage: StorageLike = getServiceStorage(),
): UserPreferences {
  const patch = loadPendingPreferencePatch(storage);
  return patch ? { ...preferences, ...patch, version: preferences.version } : preferences;
}

/** Merge the locally queued edit onto the latest server version, then clear it. */
export async function flushPendingPreferencePatch(
  api: PreferencesApi,
  storage: StorageLike = getServiceStorage(),
): Promise<UserPreferences | null> {
  const patch = loadPendingPreferencePatch(storage);
  if (!patch) return null;
  const current = await api.getPreferences();
  const saved = await api.putPreferences(applyPendingPreferencePatch(current, storage));
  storage.removeItem(PENDING_PREFERENCES_KEY);
  return saved;
}
