import { afterEach, describe, expect, it } from "vitest";
import {
  loadLocalCustomAiSettings,
  removeLocalCustomAiSettings,
  saveLocalCustomAiSettings,
  type LocalCustomAiSettings,
} from "../src/app/localAiSettings";

function settings(userId: string, apiKey: string): LocalCustomAiSettings {
  return {
    userId,
    baseUrl: "https://provider.example/v1",
    model: "model-a",
    apiKey,
    routingPreset: "single",
  };
}

describe("local custom AI credential scoping", () => {
  afterEach(() => {
    sessionStorage.clear();
  });

  it("preserves separate credentials per service and account", async () => {
    await saveLocalCustomAiSettings(settings("u_a", "key-a"), "https://one.example");
    await saveLocalCustomAiSettings(settings("u_b", "key-b"), "https://one.example");
    await saveLocalCustomAiSettings(settings("u_a", "key-a-two"), "https://two.example");

    await expect(
      loadLocalCustomAiSettings("u_a", "https://one.example/"),
    ).resolves.toMatchObject({ apiKey: "key-a" });
    await expect(
      loadLocalCustomAiSettings("u_b", "https://one.example"),
    ).resolves.toMatchObject({ apiKey: "key-b" });
    await expect(
      loadLocalCustomAiSettings("u_a", "https://two.example"),
    ).resolves.toMatchObject({ apiKey: "key-a-two" });
  });

  it("removes only the selected service/account credential", async () => {
    await saveLocalCustomAiSettings(settings("u_a", "key-a"), "https://one.example");
    await saveLocalCustomAiSettings(settings("u_b", "key-b"), "https://one.example");

    await removeLocalCustomAiSettings("u_b", "https://one.example");

    await expect(
      loadLocalCustomAiSettings("u_a", "https://one.example"),
    ).resolves.toMatchObject({ apiKey: "key-a" });
    await expect(
      loadLocalCustomAiSettings("u_b", "https://one.example"),
    ).resolves.toBeNull();
  });

  it("migrates the legacy single-account browser record into the scoped store", async () => {
    sessionStorage.setItem(
      "mpgs.ai.custom.session.v2",
      JSON.stringify(settings("legacy-user", "legacy-key")),
    );

    await expect(
      loadLocalCustomAiSettings("legacy-user", "https://service.example"),
    ).resolves.toMatchObject({ apiKey: "legacy-key" });

    expect(sessionStorage.getItem("mpgs.ai.custom.session.v2")).toBeNull();
    expect(sessionStorage.getItem("mpgs.ai.custom.session.v3")).toContain('"v":3');
  });
});
