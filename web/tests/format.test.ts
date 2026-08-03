import { describe, expect, it } from "vitest";
import {
  dominantModeLabel,
  evidenceValueLabel,
  formatPrice,
  hasConcretePartySize,
  isStale,
  languageLabels,
  partyLabel,
  platformLabels,
  positiveRate,
  recommendationFitBandLabel,
  recommendationLabel,
  recommendationSlotReasonLabel,
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

  it("renders recommendation rank and index without implying a percentage", () => {
    expect(recommendationLabel(3, 86, 0.8)).toBe("第 3 推荐 · 推荐指数 86");
    expect(recommendationLabel(3, 86, 0.8, "excellent")).toBe(
      "第 3 推荐 · 很适合 · 推荐指数 86",
    );
    expect(recommendationLabel(null, 86.4, 0.8)).toBe("推荐指数 86");
    expect(recommendationLabel(2, null, 0.8)).toBe("第 2 推荐 · 资料较少，待观察");
    expect(recommendationLabel(2, 86, 0.44)).toBe("第 2 推荐 · 资料较少，待观察");
    expect(recommendationLabel(2, 86, undefined)).toBe("第 2 推荐 · 资料较少，待观察");
    expect(recommendationLabel(2, 86, 0.9, "insufficient_data")).toBe(
      "第 2 推荐 · 资料较少，待观察",
    );
  });

  it("maps only supported recommendation labels", () => {
    expect(recommendationFitBandLabel("excellent")).toBe("很适合");
    expect(recommendationFitBandLabel("good")).toBe("适合");
    expect(recommendationFitBandLabel("consider")).toBe("值得考虑");
    expect(recommendationFitBandLabel("insufficient_data")).toBe("资料较少");
    expect(recommendationFitBandLabel("future_value")).toBeNull();
    expect(recommendationSlotReasonLabel("base")).toBe("综合适配");
    expect(recommendationSlotReasonLabel("diversity")).toBe("多样性");
    expect(recommendationSlotReasonLabel("explore")).toBe("探索");
    expect(recommendationSlotReasonLabel("future_value")).toBeNull();
  });

  it("renders review-summary evidence readably, never raw JSON", () => {
    expect(
      evidenceValueLabel({ positive: 4500, total: 5000, wilson_lower: 0.8913748542115199 }),
    ).toBe("4500/5000 好评 · 加权好评率约 89%");
    expect(evidenceValueLabel({ positive: 3, total: 10 })).toBe("3/10 好评");
    expect(evidenceValueLabel(true)).toBe("是");
    expect(evidenceValueLabel(null)).toBe("未知");
    expect(evidenceValueLabel("coop")).toBe("coop");
    // Unknown object shapes stay verbatim JSON.
    expect(evidenceValueLabel({ foo: 1 })).toBe('{"foo":1}');
  });
});
