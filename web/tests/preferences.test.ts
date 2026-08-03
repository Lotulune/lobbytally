import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyPendingPreferencePatch,
  defaultPreferences,
  editablePreferencePatch,
  flushPendingPreferencePatch,
  hasPendingPreferencePatch,
  PLATFORM_OPTIONS,
  preferencesChanged,
  queuePreferencePatch,
  setPreferenceOwnerResolver,
  SESSION_OPTIONS,
  toggleMember,
} from "../src/app/preferences";

function memoryStorage() {
  const data = new Map<string, string>();
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
    removeItem: (key: string) => void data.delete(key),
  };
}

describe("preference helpers", () => {
  afterEach(() => {
    setPreferenceOwnerResolver(null);
  });

  it("treats untouched default preferences as unconfirmed", () => {
    expect(defaultPreferences().preference_confidence).toBe(0);
  });

  it("uses the normalized macOS platform identifier", () => {
    expect(PLATFORM_OPTIONS.find((option) => option.label === "macOS")?.id).toBe("macos");
  });

  it("maps the 2–3 hour session option to 120–180 minutes", () => {
    expect(SESSION_OPTIONS.find((option) => option.label === "2–3 小时")).toEqual({
      label: "2–3 小时",
      min: 120,
      max: 180,
    });
  });

  it("reports no change for an identical copy", () => {
    const base = defaultPreferences();
    expect(preferencesChanged(base, { ...base })).toBe(false);
  });

  it("ignores version when comparing", () => {
    const base = defaultPreferences();
    expect(preferencesChanged(base, { ...base, version: base.version + 5 })).toBe(false);
  });

  it("detects when the player confirms previously uncertain preferences", () => {
    const base = defaultPreferences();
    expect(preferencesChanged(base, { ...base, preference_confidence: 1 })).toBe(true);
  });

  it("detects scalar and set changes", () => {
    const base = defaultPreferences();
    expect(preferencesChanged(base, { ...base, party_size: 6 })).toBe(true);
    expect(preferencesChanged(base, { ...base, budget_max_each_minor: null })).toBe(true);
    expect(preferencesChanged(base, { ...base, platforms: ["windows", "linux"] })).toBe(true);
  });

  it("treats set order as insignificant", () => {
    const base = { ...defaultPreferences(), languages: ["english", "schinese"] };
    expect(preferencesChanged(base, { ...base, languages: ["schinese", "english"] })).toBe(false);
  });

  it("creates a complete editable patch without the server version", () => {
    const preferences = { ...defaultPreferences(), version: 9 };
    const patch = editablePreferencePatch(preferences);

    expect(patch).toEqual(
      Object.fromEntries(Object.entries(preferences).filter(([key]) => key !== "version")),
    );
    expect("version" in patch).toBe(false);
  });

  it("toggles membership immutably", () => {
    const list = ["windows"];
    const added = toggleMember(list, "linux");
    expect(added).toEqual(["windows", "linux"]);
    expect(toggleMember(added, "windows")).toEqual(["linux"]);
    expect(list).toEqual(["windows"]); // original untouched
  });

  it("persists a pending patch and merges it onto the latest server version", async () => {
    const storage = memoryStorage();
    const current = { ...defaultPreferences(), version: 7, languages: ["english"] };
    const putPreferences = vi.fn(async (preferences) => ({ ...preferences, version: 8 }));
    const api = { getPreferences: vi.fn(async () => current), putPreferences };

    expect(queuePreferencePatch({ party_size: 6, budget_max_each_minor: null }, storage)).toBe(true);
    expect(hasPendingPreferencePatch(storage)).toBe(true);

    const saved = await flushPendingPreferencePatch(api, storage);
    expect(putPreferences).toHaveBeenCalledWith({
      ...current,
      party_size: 6,
      budget_max_each_minor: null,
      version: 7,
    });
    expect(saved?.version).toBe(8);
    expect(hasPendingPreferencePatch(storage)).toBe(false);
  });

  it("applies a pending patch to the local settings draft", () => {
    const storage = memoryStorage();
    queuePreferencePatch({ party_size: 8, coop_competitive: 0.9 }, storage);

    expect(applyPendingPreferencePatch(defaultPreferences(), storage)).toMatchObject({
      version: 1,
      party_size: 8,
      coop_competitive: 0.9,
    });
  });

  it("keeps a pending patch when synchronization fails", async () => {
    const storage = memoryStorage();
    queuePreferencePatch({ party_size: 8 }, storage);
    const api = {
      getPreferences: vi.fn(async () => defaultPreferences()),
      putPreferences: vi.fn(async () => {
        throw new Error("offline");
      }),
    };

    await expect(flushPendingPreferencePatch(api, storage)).rejects.toThrow("offline");
    expect(hasPendingPreferencePatch(storage)).toBe(true);
  });

  it("lets the first authenticated account claim an onboarding patch", async () => {
    const storage = memoryStorage();
    let currentUserId: string | null = null;
    setPreferenceOwnerResolver(() => currentUserId);
    queuePreferencePatch({ party_size: 6 }, storage);

    currentUserId = "u_first";
    const putPreferences = vi.fn(async (preferences: ReturnType<typeof defaultPreferences>) => ({
      ...preferences,
      version: preferences.version + 1,
    }));
    const api = {
      sessionUserId: () => currentUserId,
      isAccountAuthenticated: () => currentUserId !== null,
      getPreferences: vi.fn(async () => defaultPreferences()),
      putPreferences,
    };

    await flushPendingPreferencePatch(api, storage);

    expect(putPreferences).toHaveBeenCalledWith(
      expect.objectContaining({ party_size: 6, version: 1 }),
    );
    expect(hasPendingPreferencePatch(storage)).toBe(false);
  });

  it("keeps different accounts' pending patches isolated", async () => {
    const storage = memoryStorage();
    let currentUserId: string | null = "u_a";
    setPreferenceOwnerResolver(() => currentUserId);
    queuePreferencePatch({ party_size: 6 }, storage);

    currentUserId = "u_b";
    expect(hasPendingPreferencePatch(storage)).toBe(false);
    queuePreferencePatch({ party_size: 8 }, storage);
    const api = {
      sessionUserId: () => currentUserId,
      isAccountAuthenticated: () => true,
      getPreferences: vi.fn(async () => defaultPreferences()),
      putPreferences: vi.fn(async (preferences: ReturnType<typeof defaultPreferences>) => ({
        ...preferences,
        version: 2,
      })),
    };
    await flushPendingPreferencePatch(api, storage);
    expect(api.putPreferences).toHaveBeenCalledWith(
      expect.objectContaining({ party_size: 8 }),
    );

    currentUserId = "u_a";
    expect(hasPendingPreferencePatch(storage)).toBe(true);
    expect(applyPendingPreferencePatch(defaultPreferences(), storage).party_size).toBe(6);
  });

  it("does not delete a newer patch queued while an older PUT is in flight", async () => {
    const storage = memoryStorage();
    setPreferenceOwnerResolver(() => "u_a");
    queuePreferencePatch({ party_size: 6 }, storage);
    let resolvePut!: (value: ReturnType<typeof defaultPreferences>) => void;
    const putResult = new Promise<ReturnType<typeof defaultPreferences>>((resolve) => {
      resolvePut = resolve;
    });
    const api = {
      sessionUserId: () => "u_a",
      isAccountAuthenticated: () => true,
      getPreferences: vi.fn(async () => defaultPreferences()),
      putPreferences: vi.fn(() => putResult),
    };
    const flushing = flushPendingPreferencePatch(api, storage);
    await vi.waitFor(() => expect(api.putPreferences).toHaveBeenCalledTimes(1));

    queuePreferencePatch({ party_size: 8 }, storage);
    resolvePut({ ...defaultPreferences(), party_size: 6, version: 2 });
    await flushing;

    expect(hasPendingPreferencePatch(storage)).toBe(true);
    expect(applyPendingPreferencePatch(defaultPreferences(), storage).party_size).toBe(8);
  });
});
