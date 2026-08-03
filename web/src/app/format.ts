// Presentation helpers. Unknown data must render as 未知, never as a guess
// (PRD 体验原则: 数据未知时显示未知).

import type { FeedSection } from "../api/types";

export const SECTION_META: Record<FeedSection, { label: string; hint: string }> = {
  recent_release: { label: "近期正式发售", hint: "按最早已知发售日，近 180 天内的正式发售" },
  upcoming: { label: "即将发售 / Demo", hint: "Steam 发售日历 · 未来 30 天" },
  popular_legacy: { label: "人气老游", hint: "仍活跃且口碑达标的老游" },
  classic_legacy: { label: "老牌联机", hint: "热门之外、发售超过 180 天的多人游戏" },
};

const DOMINANT_MODE_LABELS: Record<string, string> = {
  private_coop: "私人合作",
  coop: "合作",
  online_coop: "在线合作",
  self_hosted: "自建服务器",
  self_hosted_survival: "自建服生存",
  dedicated_server: "专用服务器",
  p2p: "P2P 联机",
  matchmaking: "公共匹配",
  matchmaking_core: "公共匹配核心",
  matchmaking_competitive: "竞技匹配",
  mmo: "MMO",
  pvp: "对抗",
  competitive: "对抗",
  versus: "对抗",
  // Both co-op and competitive tags present (Steam categories / mixed profile).
  mixed: "合作/对抗",
  multiplayer: "联机",
  unknown: "未知",
};

export function dominantModeLabel(mode: string | null | undefined): string {
  if (!mode) return "未知";
  const key = mode.trim().toLowerCase();
  if (!key || key === "unknown") return "未知";
  return DOMINANT_MODE_LABELS[key] ?? DOMINANT_MODE_LABELS[mode] ?? mode;
}

/**
 * Party-size label for UI.
 *
 * Most catalog rows only have a store-search placeholder `recommended_min=2`
 * with no max. That is not useful ("至少 2 人" ≈ multiplayer tag), so we only
 * show concrete numbers when max is known (or a true min–max range).
 */
export function partyLabel(min: number | null, max: number | null): string {
  if (min !== null && max !== null) {
    return min === max ? `${min} 人` : `${min}–${max} 人`;
  }
  if (max !== null) return `最多 ${max} 人`;
  // min-only (or neither) → unknown / not yet evidenced
  return "人数未定";
}

/** True when we have a concrete party bound worth showing as a chip (needs max). */
export function hasConcretePartySize(_min: number | null, max: number | null): boolean {
  return max !== null;
}

export function formatPrice(minor: number | null, currency: string | null, isFree: boolean | null): string {
  if (isFree === true) return "免费";
  if (minor === null || !currency) return "价格未知";
  const major = minor / 100;
  const symbol = currency === "CNY" ? "¥" : currency === "USD" ? "$" : `${currency} `;
  return `${symbol}${major % 1 === 0 ? major.toFixed(0) : major.toFixed(2)}`;
}

export function formatPercent(value: number | null): string {
  if (value === null || Number.isNaN(value)) return "未知";
  return `${Math.round(value * 100)}%`;
}

const RECOMMENDATION_CONFIDENCE_THRESHOLD = 0.45;

/**
 * Player-facing recommendation label. The index is a relative rank within the
 * current context, not a probability, so it deliberately has no percent sign.
 */
export function recommendationLabel(
  rank: number | null | undefined,
  index: number | null | undefined,
  dataConfidence: number | null | undefined,
  fitBand?: string | null,
): string {
  const validRank = typeof rank === "number" && Number.isFinite(rank) && rank >= 1
    ? Math.round(rank)
    : null;
  const hasEnoughData =
    fitBand !== "insufficient_data" &&
    typeof index === "number" &&
    Number.isFinite(index) &&
    typeof dataConfidence === "number" &&
    Number.isFinite(dataConfidence) &&
    dataConfidence >= RECOMMENDATION_CONFIDENCE_THRESHOLD;
  const indexLabel = hasEnoughData
    ? `推荐指数 ${Math.round(Math.min(100, Math.max(0, index)))}`
    : "资料较少，待观察";
  const fitLabel = hasEnoughData ? recommendationFitBandLabel(fitBand) : null;
  return [validRank === null ? null : `第 ${validRank} 推荐`, fitLabel, indexLabel]
    .filter(Boolean)
    .join(" · ");
}

export function hasLowRecommendationConfidence(
  dataConfidence: number | null | undefined,
): boolean {
  return (
    typeof dataConfidence === "number" &&
    Number.isFinite(dataConfidence) &&
    dataConfidence < RECOMMENDATION_CONFIDENCE_THRESHOLD
  );
}

/** Keep evidence quality visible independently from the recommendation index. */
export function recommendationDataReliabilityLabel(
  dataConfidence: number | null | undefined,
): string {
  if (
    typeof dataConfidence !== "number" ||
    !Number.isFinite(dataConfidence)
  ) {
    return "数据可靠度未知";
  }
  const percentage = Math.round(Math.min(1, Math.max(0, dataConfidence)) * 100);
  if (dataConfidence >= 0.8) return `数据可靠度高 · ${percentage}%`;
  if (dataConfidence >= RECOMMENDATION_CONFIDENCE_THRESHOLD) {
    return `数据可靠度中等 · ${percentage}%`;
  }
  return `数据可靠度低 · ${percentage}%`;
}

const FIT_BAND_LABELS: Record<string, string> = {
  excellent: "很适合",
  good: "适合",
  consider: "值得考虑",
  insufficient_data: "资料较少",
};

export function recommendationFitBandLabel(band: string | null | undefined): string | null {
  if (!band) return null;
  return FIT_BAND_LABELS[band] ?? null;
}

const SLOT_REASON_LABELS: Record<string, string> = {
  base: "综合适配",
  diversity: "多样性",
  explore: "探索",
};

export function recommendationSlotReasonLabel(reason: string | null | undefined): string | null {
  if (!reason) return null;
  return SLOT_REASON_LABELS[reason] ?? null;
}

export function formatCount(value: number | null): string {
  if (value === null) return "未知";
  if (value >= 10_000) return `${(value / 10_000).toFixed(1)} 万`;
  return String(value);
}

export function positiveRate(total: number | null, positive: number | null): string {
  if (!total || positive === null) return "未知";
  return `${Math.round((positive / total) * 100)}% 好评`;
}

export function formatAgo(ms: number, now: number = Date.now()): string {
  const delta = Math.max(0, now - ms);
  const minutes = Math.floor(delta / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天前`;
  const months = Math.floor(days / 30);
  return `${months} 个月前`;
}

/** Data older than this renders a staleness warning (NFR-004). */
export const STALE_AFTER_MS = 24 * 60 * 60 * 1000;

export function isStale(dataUpdatedAtMs: number, now: number = Date.now()): boolean {
  return now - dataUpdatedAtMs > STALE_AFTER_MS;
}

const RELEASE_STATE_LABELS: Record<string, string> = {
  released: "已发售",
  coming_soon: "即将发售",
  early_access: "抢先体验",
  unreleased: "未发售",
  demo: "试玩",
  playtest: "Playtest",
  delisted: "已下架",
};

export function releaseStateLabel(state: string): string {
  return RELEASE_STATE_LABELS[state] ?? state;
}

/** Preserve the source precision rather than inventing a calendar day. */
export function formatReleaseDate(
  date: string | null,
  raw: string | null,
  precision: string | null,
): string {
  if (precision === "tba" || (!date && !raw)) return "日期未定";
  const source = date ?? raw ?? "";
  if (precision === "day") {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(source);
    if (match) return `${match[1]} 年${Number(match[2])} 月${Number(match[3])} 日`;
    return raw ?? "日期未定";
  }
  if (precision === "month") {
    const match = /^(\d{4})-(\d{2})/.exec(source);
    if (match) return `预计 ${match[1]} 年${Number(match[2])} 月`;
    return raw ?? "日期未定";
  }
  if (precision === "quarter") {
    const match = /(\d{4}).*?(?:Q|第\s*)([1-4])/i.exec(source);
    if (match) return `预计 ${match[1]} 年第${match[2]}季度`;
    return raw ?? "日期未定";
  }
  if (precision === "year") {
    const match = /(\d{4})/.exec(source);
    if (match) return `预计 ${match[1]} 年`;
    return raw ?? "日期未定";
  }
  return raw ?? (source || "日期未定");
}

const PLATFORM_LABELS: Record<string, string> = {
  windows: "Windows",
  mac: "macOS",
  macos: "macOS",
  linux: "Linux",
  steamdeck: "Steam Deck",
};

export function platformLabels(platforms: string[]): string {
  if (platforms.length === 0) return "未知";
  return platforms.map((p) => PLATFORM_LABELS[p] ?? p).join(" / ");
}

const LANGUAGE_LABELS: Record<string, string> = {
  arabic: "阿拉伯语",
  brazilian: "巴西葡萄牙语",
  bulgarian: "保加利亚语",
  czech: "捷克语",
  danish: "丹麦语",
  dutch: "荷兰语",
  schinese: "简体中文",
  tchinese: "繁体中文",
  english: "英语",
  finnish: "芬兰语",
  japanese: "日语",
  koreana: "韩语",
  russian: "俄语",
  german: "德语",
  french: "法语",
  greek: "希腊语",
  hungarian: "匈牙利语",
  indonesian: "印度尼西亚语",
  italian: "意大利语",
  norwegian: "挪威语",
  polish: "波兰语",
  portuguese: "葡萄牙语",
  romanian: "罗马尼亚语",
  spanish: "西班牙语",
  swedish: "瑞典语",
  thai: "泰语",
  turkish: "土耳其语",
  latam: "拉美西语",
  ukrainian: "乌克兰语",
  vietnamese: "越南语",
};

export function languageLabels(languages: string[]): string {
  if (languages.length === 0) return "未知";
  return languages.map((l) => LANGUAGE_LABELS[l] ?? l).join("、");
}

export const FEEDBACK_LABELS: Record<string, string> = {
  like: "喜欢",
  not_interested: "不感兴趣",
  played: "玩过",
  too_competitive: "太竞技",
  party_size_mismatch: "人数不合适",
  hosting_friction: "开服麻烦",
};

const FEATURE_LABELS: Record<string, string> = {
  private_session: "私人房间",
  online_coop: "在线合作",
  self_hosted_server: "自建服务器",
  multiplayer_category: "多人分类",
  dedicated_server: "专用服务器",
  drop_in_out: "随进随出",
};

export function featureLabel(feature: string): string {
  return FEATURE_LABELS[feature] ?? feature;
}

const SOURCE_TYPE_LABELS: Record<string, string> = {
  official_api: "官方接口",
  official_store: "官方商店",
  store_adapter: "商店适配器",
  manual: "人工确认",
  inference: "推断",
  ai_inference: "AI 推断",
};

export function sourceTypeLabel(type: string): string {
  return SOURCE_TYPE_LABELS[type] ?? type;
}

export function evidenceValueLabel(value: unknown): string {
  if (value === true) return "是";
  if (value === false) return "否";
  if (value === null || value === undefined) return "未知";
  if (typeof value === "number" || typeof value === "string") return String(value);
  // Review-summary objects (positive/total[/wilson_lower]) get a readable
  // rendering instead of raw JSON; anything else stays verbatim JSON.
  if (typeof value === "object") {
    const summary = value as { positive?: unknown; total?: unknown; wilson_lower?: unknown };
    if (typeof summary.positive === "number" && typeof summary.total === "number") {
      const wilson =
        typeof summary.wilson_lower === "number"
          ? ` · 加权好评率约 ${Math.round(summary.wilson_lower * 100)}%`
          : "";
      return `${summary.positive}/${summary.total} 好评${wilson}`;
    }
  }
  return JSON.stringify(value);
}
