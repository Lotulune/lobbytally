import { describe, expect, it } from "vitest";
import {
  dominantModeLabel,
  formatPrice,
  hasConcretePartySize,
  isStale,
  languageLabels,
  partyLabel,
  platformLabels,
  positiveRate,
  SECTION_META,
  STALE_AFTER_MS,
} from "../src/app/format";

describe("format helpers", () => {
  it("renders unknown data as 未知, never a guess", () => {
    expect(dominantModeLabel(null)).toBe("未知");
    expect(dominantModeLabel("")).toBe("未知");
    expect(dominantModeLabel("unknown")).toBe("未知");
    expect(partyLabel(null, null)).toBe("人数未定");
    expect(formatPrice(null, null, null)).toBe("价格未知");
    expect(platformLabels([])).toBe("未知");
    expect(languageLabels([])).toBe("未知");
    expect(positiveRate(null, null)).toBe("未知");
  });

  it("formats known values", () => {
    expect(partyLabel(1, 4)).toBe("1–4 人");
    expect(partyLabel(4, 4)).toBe("4 人");
    expect(partyLabel(null, 8)).toBe("最多 8 人");
    // Placeholder min-only from store multiplayer category must not look precise.
    expect(partyLabel(2, null)).toBe("人数未定");
    expect(hasConcretePartySize(2, null)).toBe(false);
    expect(hasConcretePartySize(1, 4)).toBe(true);
    expect(hasConcretePartySize(null, 8)).toBe(true);
    expect(formatPrice(0, "CNY", true)).toBe("免费");
    expect(formatPrice(14900, "CNY", false)).toBe("¥149");
    expect(platformLabels(["windows", "linux"])).toBe("Windows / Linux");
    expect(languageLabels(["brazilian", "italian", "polish"])).toBe("巴西葡萄牙语、意大利语、波兰语");
    expect(dominantModeLabel("self_hosted_survival")).toBe("自建服生存");
    expect(dominantModeLabel("coop")).toBe("合作");
    expect(dominantModeLabel("pvp")).toBe("对抗");
    expect(dominantModeLabel("competitive")).toBe("对抗");
    // Both coop and competitive tags → 合作/对抗 (not vague 混合).
    expect(dominantModeLabel("mixed")).toBe("合作/对抗");
    expect(dominantModeLabel("multiplayer")).toBe("联机");
    expect(SECTION_META.recent_release).toEqual({
      label: "近期正式发售",
      hint: "按最早已知发售日，近 180 天内的正式发售",
    });
    expect(positiveRate(100, 90)).toBe("90% 好评");
  });

  it("flags data older than the staleness window", () => {
    const now = 1_000_000_000_000;
    expect(isStale(now, now)).toBe(false);
    expect(isStale(now - STALE_AFTER_MS - 1, now)).toBe(true);
  });
});
