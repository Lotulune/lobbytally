// Preference form helpers: option lists, defaults, and change detection so the
// settings screen only PUTs when something actually differs.

import type { StorageLike, UserPreferences } from "../api/types";
import { getServiceStorage } from "../api/storage";

const PENDING_PREFERENCES_KEY = "mpgs.preferences.pending.v1";

export type PendingPreferencesPatch = Omit<Partial<UserPreferences>, "version">;

interface PreferencesApi {
  getPreferences(): Promise<UserPreferences>;
  putPreferences(preferences: UserPreferences): Promise<UserPreferences>;
  sessionUserId?(): string | null;
  isAccountAuthenticated?(): boolean;
}

interface PendingPreferencesEntry {
  id: string;
  ownerUserId: string | null;
  /** Onboarding creates this before an account exists; the next account claims it. */
  claimOnNextAccount: boolean;
  patch: PendingPreferencesPatch;
}

interface PendingPreferencesStore {
  v: 2;
  entries: PendingPreferencesEntry[];
}

let preferenceOwnerResolver: (() => string | null) | null = null;

/** Runtime hook: bind pending preferences to the active account identity. */
export function setPreferenceOwnerResolver(resolver: (() => string | null) | null): void {
  preferenceOwnerResolver = resolver;
}

function resolvedOwner(): { known: boolean; userId: string | null } {
  if (!preferenceOwnerResolver) return { known: false, userId: null };
  try {
    return { known: true, userId: preferenceOwnerResolver() };
  } catch {
    return { known: true, userId: null };
  }
}

function apiOwner(api: PreferencesApi): { known: boolean; userId: string | null } {
  if (typeof api.sessionUserId === "function") {
    const authenticated =
      typeof api.isAccountAuthenticated !== "function" || api.isAccountAuthenticated();
    return { known: true, userId: authenticated ? api.sessionUserId() : null };
  }
  return resolvedOwner();
}

function newPendingPreferencesId(): string {
  const bytes = new Uint8Array(12);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function normalizeEntry(value: unknown): PendingPreferencesEntry | null {
  if (!isRecord(value) || !isRecord(value.patch)) return null;
  if (typeof value.id !== "string") return null;
  return {
    id: value.id,
    ownerUserId: typeof value.ownerUserId === "string" ? value.ownerUserId : null,
    claimOnNextAccount: value.claimOnNextAccount === true,
    patch: value.patch as PendingPreferencesPatch,
  };
}

function writePendingPreferenceEntries(
  storage: StorageLike,
  entries: PendingPreferencesEntry[],
): void {
  if (entries.length === 0) {
    storage.removeItem(PENDING_PREFERENCES_KEY);
    return;
  }
  const value: PendingPreferencesStore = { v: 2, entries };
  storage.setItem(PENDING_PREFERENCES_KEY, JSON.stringify(value));
}

function loadPendingPreferenceEntries(storage: StorageLike): PendingPreferencesEntry[] {
  try {
    const raw = storage.getItem(PENDING_PREFERENCES_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (isRecord(parsed) && parsed.v === 2 && Array.isArray(parsed.entries)) {
      return parsed.entries.map(normalizeEntry).filter((entry): entry is PendingPreferencesEntry => entry !== null);
    }
    if (!isRecord(parsed)) return [];

    // Legacy v1 stored a raw patch. Adopt it only when the same persisted
    // account is active during migration; otherwise quarantine it. In generic
    // test/embedding environments without an identity provider, preserve the
    // historical "next account" behavior.
    const owner = resolvedOwner();
    const migrated: PendingPreferencesEntry = {
      id: newPendingPreferencesId(),
      ownerUserId: owner.userId,
      claimOnNextAccount: !owner.known,
      patch: parsed as PendingPreferencesPatch,
    };
    writePendingPreferenceEntries(storage, [migrated]);
    return [migrated];
  } catch {
    return [];
  }
}

function visiblePreferenceEntry(
  entries: PendingPreferencesEntry[],
  owner = resolvedOwner(),
): PendingPreferencesEntry | null {
  if (owner.userId) {
    return (
      entries.find((entry) => entry.ownerUserId === owner.userId) ??
      entries.find((entry) => entry.claimOnNextAccount) ??
      null
    );
  }
  if (!owner.known) {
    return entries.find((entry) => entry.claimOnNextAccount) ?? entries[0] ?? null;
  }
  return entries.find((entry) => entry.claimOnNextAccount) ?? null;
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
    preference_confidence: 0,
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
    base.preference_confidence !== next.preference_confidence ||
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
    preference_confidence: preferences.preference_confidence,
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
    const entries = loadPendingPreferenceEntries(storage);
    const owner = resolvedOwner();
    const claimOnNextAccount = owner.userId === null && !owner.known;
    // A configured runtime with no account is the onboarding flow: its patch is
    // intentionally claimed by the next authenticated account.
    const shouldClaim = claimOnNextAccount || (owner.known && owner.userId === null);
    const next: PendingPreferencesEntry = {
      id: newPendingPreferencesId(),
      ownerUserId: owner.userId,
      claimOnNextAccount: shouldClaim,
      patch,
    };
    const kept = entries.filter((entry) =>
      owner.userId
        ? entry.ownerUserId !== owner.userId
        : !entry.claimOnNextAccount,
    );
    writePendingPreferenceEntries(storage, [...kept, next]);
    return true;
  } catch {
    return false;
  }
}

export function hasPendingPreferencePatch(
  storage: StorageLike = getServiceStorage(),
): boolean {
  try {
    return visiblePreferenceEntry(loadPendingPreferenceEntries(storage)) !== null;
  } catch {
    return false;
  }
}

export function applyPendingPreferencePatch(
  preferences: UserPreferences,
  storage: StorageLike = getServiceStorage(),
): UserPreferences {
  const entry = visiblePreferenceEntry(loadPendingPreferenceEntries(storage));
  return entry ? { ...preferences, ...entry.patch, version: preferences.version } : preferences;
}

/** Merge the locally queued edit onto the latest server version, then clear it. */
export async function flushPendingPreferencePatch(
  api: PreferencesApi,
  storage: StorageLike = getServiceStorage(),
): Promise<UserPreferences | null> {
  const owner = apiOwner(api);
  if (owner.known && !owner.userId) return null;
  let entries = loadPendingPreferenceEntries(storage);
  let entry = visiblePreferenceEntry(entries, owner);
  if (!entry) return null;

  if (owner.userId && entry.claimOnNextAccount) {
    entry = { ...entry, ownerUserId: owner.userId, claimOnNextAccount: false };
    entries = entries.map((candidate) => (candidate.id === entry?.id ? entry : candidate));
    writePendingPreferenceEntries(storage, entries);
  }

  const current = await api.getPreferences();
  const beforePutOwner = apiOwner(api);
  if (
    owner.known &&
    (beforePutOwner.userId === null || beforePutOwner.userId !== entry.ownerUserId)
  ) {
    return null;
  }
  const saved = await api.putPreferences({
    ...current,
    ...entry.patch,
    version: current.version,
  });
  // Do not delete a newer patch queued while this request was in flight.
  const latest = loadPendingPreferenceEntries(storage);
  writePendingPreferenceEntries(
    storage,
    latest.filter((candidate) => candidate.id !== entry.id),
  );
  return saved;
}
