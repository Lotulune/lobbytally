// Client-side helpers for custom multi-model routing (device-local keys only).

export type CustomRoutingPreset = "easy" | "single";

export interface CustomTaskRoute {
  task: string;
  primary_model: string;
  fallback_models: string[];
}

export interface EasyRoutePlan {
  preset: CustomRoutingPreset;
  /** Default model used when a task has no override (also shown as "主模型"). */
  model: string;
  fallback_model: string | null;
  routes: CustomTaskRoute[];
  notes: string[];
}

const IMAGE_LIKE = /imagine|image|dall|vision|tts|whisper|embed|moderation/i;
const FAST_LIKE = /fast|mini|flash|lite|nano|haiku|chat-fast|turbo|small/i;
const REASON_LIKE = /reason|o1|o3|thinking|pro-reason/i;
const STRONG_LIKE = /4\.5|4\.3|4o|opus|sonnet|gpt-4|claude-3|grok-4(?!\.20)/i;

function isChatCandidate(id: string): boolean {
  return !IMAGE_LIKE.test(id);
}

function pickFirst(ids: string[], pred: (id: string) => boolean): string | undefined {
  return ids.find(pred);
}

function unique(ids: string[]): string[] {
  return [...new Set(ids.filter(Boolean))];
}

/**
 * Build a user-friendly multi-model plan from an upstream `/v1/models` list.
 * Prefer fast models for intent, stronger ones for rank/compare.
 */
export function buildEasyRoutePlan(modelIds: string[]): EasyRoutePlan {
  const chat = unique(modelIds.map((m) => m.trim()).filter(isChatCandidate));
  const notes: string[] = [];

  if (chat.length === 0) {
    return {
      preset: "single",
      model: modelIds[0] ?? "gpt-4o-mini",
      fallback_model: null,
      routes: [],
      notes: ["未发现可用聊天模型，请手动填写一个模型名。"],
    };
  }

  const fast = pickFirst(chat, (id) => FAST_LIKE.test(id));
  const reason = pickFirst(chat, (id) => REASON_LIKE.test(id));
  const strong: string =
    pickFirst(chat, (id) => STRONG_LIKE.test(id) && !FAST_LIKE.test(id)) ??
    pickFirst(chat, (id) => !FAST_LIKE.test(id)) ??
    chat[0]!;
  const mid: string =
    pickFirst(chat, (id) => id !== strong && id !== fast && !REASON_LIKE.test(id)) ?? strong;

  if (chat.length === 1) {
    notes.push("上游只有一个聊天模型，所有任务共用它（仍支持任务级超时与协议探测）。");
    const only = chat[0]!;
    return {
      preset: "single",
      model: only,
      fallback_model: null,
      routes: singleModelRoutes(only),
      notes,
    };
  }

  const intentPrimary: string = fast ?? mid;
  const intentFallback = unique([mid, strong].filter((m) => m !== intentPrimary));
  const rankPrimary = strong;
  const rankFallback = unique(
    [mid, fast, reason].filter((m): m is string => typeof m === "string" && m !== rankPrimary),
  );
  const comparePrimary: string = reason ?? strong;
  const compareFallback = unique([strong, mid].filter((m) => m !== comparePrimary));

  notes.push("已按「快意图 / 强推荐 / 推理比较」自动分配主模型与回退。");
  if (fast) notes.push(`意图解析优先使用较快模型：${fast}`);
  if (reason) notes.push(`比较与小组建议优先使用推理模型：${reason}`);

  return {
    preset: "easy",
    model: rankPrimary,
    fallback_model: rankFallback[0] ?? null,
    routes: [
      {
        task: "intent_parse",
        primary_model: intentPrimary,
        fallback_models: intentFallback.slice(0, 2),
      },
      {
        task: "rank_explain",
        primary_model: rankPrimary,
        fallback_models: rankFallback.slice(0, 2),
      },
      {
        task: "compare_games",
        primary_model: comparePrimary,
        fallback_models: compareFallback.slice(0, 2),
      },
      {
        task: "group_advice",
        primary_model: comparePrimary,
        fallback_models: compareFallback.slice(0, 2),
      },
      {
        task: "game_summary",
        primary_model: mid,
        fallback_models: unique([strong].filter((m) => m !== mid)).slice(0, 1),
      },
    ],
    notes,
  };
}

function singleModelRoutes(model: string): CustomTaskRoute[] {
  return ["intent_parse", "rank_explain", "compare_games", "group_advice", "game_summary"].map(
    (task) => ({
      task,
      primary_model: model,
      fallback_models: [],
    }),
  );
}

/** Human labels for task ids shown in settings. */
export const TASK_LABELS: Record<string, string> = {
  intent_parse: "理解你的话",
  rank_explain: "推荐理由",
  compare_games: "游戏比较",
  group_advice: "小组建议",
  game_summary: "游戏总结",
  data_quality: "数据质检",
};

export function taskLabel(task: string): string {
  return TASK_LABELS[task] ?? task;
}
