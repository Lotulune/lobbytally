// Natural-language recommendations: describe the session in plain words, see
// what the server understood (interpreted constraints + ai_status), and browse
// deterministic results even when AI is disabled or fell back to rules.
//
// Load-bearing copy (asserted by E2E / product contract, do not reword):
// - submit button "推荐", input id "nl-input"
// - fallback chip "规则解析模式"
// - note "当前由确定性规则理解输入；无法识别的条件不会被伪造成已理解。"
// - offline error "离线时无法生成新的自然语言推荐。"

import { useEffect, useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { NaturalLanguageRecommendationResponse } from "../api/types";
import { formatAgo, formatPrice, isStale, platformLabels } from "../app/format";
import {
  apiClient,
  feedbackQueue,
  localCredentialServiceOrigin,
} from "../app/runtime";
import { loadLocalCustomAiSettings } from "../app/localAiSettings";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { EmptyState } from "../components/EmptyState";
import { Skeleton } from "../components/Skeleton";
import { GameCard } from "./GameCard";

const EXAMPLES = ["4 人合作，单局一小时以内", "想找能自建服务器的长期联机游戏", "两个人轻松玩，不要太竞技"];

function AiStatusChip({ result }: { result: NaturalLanguageRecommendationResponse }) {
  const reason = result.fallback_reason ?? undefined;
  switch (result.ai_status) {
    case "pending":
      return (
        <Chip tone="accent" title={reason}>
          AI 增强中
        </Chip>
      );
    case "used":
      return <Chip tone="accent">AI 已增强</Chip>;
    case "cached":
      return <Chip tone="accent">AI 缓存命中</Chip>;
    case "fallback":
      return (
        <Chip tone="warn" title={reason}>
          规则解析模式
        </Chip>
      );
    case "disabled":
      return (
        <Chip tone="warn" title={reason}>
          AI 未启用
        </Chip>
      );
    default:
      return (
        <Chip tone="warn" title={reason}>
          AI 状态未知
        </Chip>
      );
  }
}

/** Human-readable chips for the constraints the server actually understood. */
function constraintLabels(result: NaturalLanguageRecommendationResponse): string[] {
  const interpreted = result.interpreted;
  const hard = new Set(interpreted.hard_constraints ?? []);
  const represented = new Set<string>();
  const labels: string[] = [];
  const add = (field: string, label: string) => {
    represented.add(field);
    labels.push(hard.has(field) ? `${label}（硬性）` : label);
  };
  if (interpreted.party_size !== null) add("party_size", `${interpreted.party_size} 人`);
  if (interpreted.session_minutes_max !== null) {
    add("session_minutes", `最长 ${interpreted.session_minutes_max} 分钟`);
  }
  if (interpreted.coop_competitive !== null) {
    labels.push(interpreted.coop_competitive < 0.5 ? "偏合作" : "偏竞技");
  }
  if (interpreted.self_hosting_willingness != null && interpreted.self_hosting_willingness >= 0.5) {
    add("self_hosting", "自建服优先");
  }
  if (interpreted.platforms && interpreted.platforms.length > 0) {
    add("platforms", platformLabels(interpreted.platforms));
  }
  if (interpreted.demo_only) add("demo_required", "必须提供 Demo/Playtest");
  for (const mode of interpreted.modes_excluded ?? []) {
    labels.push(`排除 ${mode}`);
  }
  for (const mode of interpreted.modes_preferred ?? []) {
    labels.push(`偏好 ${mode}`);
  }
  if (interpreted.max_price_minor != null && interpreted.currency) {
    add(
      "budget",
      `预算 ≤ ${formatPrice(interpreted.max_price_minor, interpreted.currency, false)}`,
    );
  }
  const hardOnlyLabels: Record<string, string> = {
    demo_required: "必须提供 Demo",
    self_hosting: "必须支持自建服",
  };
  for (const field of hard) {
    if (!represented.has(field) && hardOnlyLabels[field]) labels.push(hardOnlyLabels[field]);
  }
  return labels;
}

function unappliedConstraintLabels(result: NaturalLanguageRecommendationResponse): string[] {
  const labels: Record<string, string> = {
    party_size: "人数条件",
    session_minutes: "单局时长",
    platforms: "平台条件",
    budget: "预算条件",
    demo_required: "Demo / Playtest 条件",
    selected_section: "推荐分区",
    coop_competitive: "合作 / 竞技偏好",
    self_hosting_preference: "开服偏好",
    modes_preferred: "玩法偏好",
    modes_excluded: "排除玩法",
  };
  return (result.interpreted.unapplied_constraints ?? []).map((value) => {
    const [field = "", detail] = value.split(":", 2);
    const label = labels[field] ?? "未理解的条件";
    return detail ? `${label}：${detail}` : label;
  });
}

export function NaturalLanguageScreen({
  onOpenGame,
}: {
  onOpenGame: (appId: number, recommendationRunId?: string | null) => void;
}) {
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<NaturalLanguageRecommendationResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);

  const run = async (override?: string) => {
    const text = (override ?? query).trim();
    if (!text || loading) return;
    setQuery(text);
    setLoading(true);
    setError(null);
    try {
      await apiClient.ensureSession();
      const userId = apiClient.isAccountAuthenticated() ? apiClient.sessionUserId() : null;
      const custom = userId
        ? await loadLocalCustomAiSettings(userId, localCredentialServiceOrigin)
        : null;
      const currentAccountUserId = apiClient.isAccountAuthenticated()
        ? apiClient.sessionUserId()
        : null;
      if (userId !== currentAccountUserId) {
        throw new ApiError({
          code: "unauthenticated",
          status: 401,
          message: "account changed while loading local AI settings",
        });
      }
      setResult(
        await apiClient.naturalLanguageRecommendations(
          text,
          6,
          custom
            ? {
                ownerUserId: custom.userId,
                provider: "openai_compat",
                baseUrl: custom.baseUrl,
                model: custom.model,
                apiKey: custom.apiKey,
                // easy/advanced: task routes from device; single: one model only.
                multiModel: custom.routingPreset !== "single",
                fallbackModel: custom.fallbackModel,
                routes: custom.routes,
              }
            : undefined,
        ),
      );
    } catch (cause) {
      setError(
        cause instanceof ApiError
          ? cause
          : new ApiError({ code: "unknown", status: 0, message: String(cause) }),
      );
    } finally {
      setLoading(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void run();
  };

  useEffect(() => {
    if (!result) return;
    return feedbackQueue.subscribeRankingChanged(() => void run(result.query));
  }, [result?.query]);

  const aiEnhanced = result !== null && (result.ai_status === "used" || result.ai_status === "cached");
  const constraints = result ? constraintLabels(result) : [];
  const unappliedConstraints = result ? unappliedConstraintLabels(result) : [];

  return (
    <section className="nl-screen" aria-label="自然语言推荐">
      <form className="nl-query" onSubmit={submit}>
        <label htmlFor="nl-input">描述这次想玩的游戏</label>
        <p className="nl-query-hint">
          说说人数、合作还是对抗、单局时长或预算；想自己开服也可以直接写。匿名可用，无需登录。
        </p>
        <div className="nl-input-row">
          <input
            id="nl-input"
            value={query}
            maxLength={500}
            placeholder="例如：4 人合作，单局一小时以内，不要太竞技"
            onChange={(event) => setQuery(event.target.value)}
          />
          <Button type="submit" variant="accent" disabled={loading || !query.trim()}>
            {loading ? "分析中" : "推荐"}
          </Button>
        </div>
        {!result && (
          <div className="nl-examples">
            <span className="nl-examples-label">试试：</span>
            {EXAMPLES.map((example) => (
              <Button key={example} size="small" variant="ghost" onClick={() => void run(example)}>
                {example}
              </Button>
            ))}
          </div>
        )}
      </form>

      {loading && !result && (
        <div className="feed-grid" aria-hidden="true">
          <Skeleton />
          <Skeleton />
          <Skeleton />
        </div>
      )}

      {error && (
        <EmptyState glyph="!" alert>
          <span>
            {error.offline
              ? "离线时无法生成新的自然语言推荐。请检查网络连接后重试。"
              : `推荐失败：${error.message}`}
          </span>
          <Button size="small" disabled={loading} onClick={() => void run()}>
            重试
          </Button>
        </EmptyState>
      )}

      {result && !error && (
        <>
          <section className="nl-analysis" aria-label="对本次描述的理解">
            <div className="nl-analysis-row">
              <span className="nl-analysis-label">AI 状态</span>
              <div className="statusline">
                <AiStatusChip result={result} />
                {result.ai_latency_ms !== undefined && aiEnhanced && (
                  <Chip>AI {result.ai_latency_ms} ms</Chip>
                )}
                {aiEnhanced && result.ai_model && <Chip>{result.ai_model}</Chip>}
              </div>
            </div>
            <div className="nl-analysis-row">
              <span className="nl-analysis-label">
                {result.interpreted.applied_constraints ? "已应用的条件" : "识别到的条件"}
              </span>
              <div className="statusline">
                {constraints.length > 0 ? (
                  constraints.map((label, index) => <Chip key={`${label}-${index}`}>{label}</Chip>)
                ) : (
                  <span className="nl-constraints-empty">未识别出明确条件，按整体偏好匹配</span>
                )}
              </div>
            </div>
            {unappliedConstraints.length > 0 && (
              <div className="nl-analysis-row">
                <span className="nl-analysis-label">尚未应用</span>
                <div className="statusline">
                  {unappliedConstraints.map((label, index) => (
                    <Chip key={`${label}-${index}`} tone="warn">
                      {label}
                    </Chip>
                  ))}
                </div>
              </div>
            )}
            <div className="nl-analysis-row">
              <span className="nl-analysis-label">数据</span>
              <div className="statusline">
                <Chip tone={isStale(result.data_updated_at_ms) ? "warn" : undefined}>
                  数据更新于 {formatAgo(result.data_updated_at_ms)}
                </Chip>
              </div>
            </div>
          </section>

          {(result.ai_status === "fallback" ||
            result.ai_status === "disabled" ||
            result.ai_status === "pending") && (
            <>
              {result.fallback_reason && <p className="cal-note">{result.fallback_reason}</p>}
              <p className="cal-note">
                当前由确定性规则理解输入；无法识别的条件不会被伪造成已理解。
              </p>
            </>
          )}
          {aiEnhanced && result.fallback_reason && <p className="cal-note">{result.fallback_reason}</p>}
          {aiEnhanced && result.ai_summary && (
            <p
              className="cal-note"
              title={result.ai_summary_evidence_ids?.length ? result.ai_summary_evidence_ids.join(", ") : undefined}
            >
              {result.ai_summary}
            </p>
          )}

          {result.items.length === 0 ? (
            <EmptyState glyph="∅">
              <span>没有找到满足已识别条件的候选。换个说法或减少条件试试。</span>
            </EmptyState>
          ) : (
            <div className="feed-grid" aria-busy={loading}>
              {result.items.map((item) => (
                <GameCard
                  key={item.app_id}
                  item={item}
                  onOpen={onOpenGame}
                  recommendationRunId={result.recommendation_run_id ?? null}
                />
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}
