import { useEffect, useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { NaturalLanguageRecommendationResponse } from "../api/types";
import { formatAgo, isStale } from "../app/format";
import { apiClient, feedbackQueue } from "../app/runtime";
import { GameCard } from "./GameCard";

const EXAMPLES = ["4 人合作，单局一小时以内", "想找能自建服务器的长期联机游戏", "两个人轻松玩，不要太竞技"];

export function NaturalLanguageScreen({ onOpenGame }: { onOpenGame: (appId: number) => void }) {
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
      setResult(await apiClient.naturalLanguageRecommendations(text));
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

  return (
    <section aria-label="自然语言推荐">
      <form className="nl-query" onSubmit={submit}>
        <label htmlFor="nl-input">描述这次想玩的游戏</label>
        <div className="search-row">
          <input
            id="nl-input"
            value={query}
            maxLength={500}
            placeholder="例如：4 人合作，单局一小时以内，不要太竞技"
            onChange={(event) => setQuery(event.target.value)}
          />
          <button type="submit" className="btn accent" disabled={loading || !query.trim()}>
            {loading ? "分析中" : "推荐"}
          </button>
        </div>
        {!result && (
          <div className="nl-examples">
            {EXAMPLES.map((example) => (
              <button key={example} type="button" className="btn small ghost" onClick={() => void run(example)}>
                {example}
              </button>
            ))}
          </div>
        )}
      </form>

      {error && (
        <div className="state-box" role="alert">
          <span className="big">!</span>
          <span>{error.offline ? "离线时无法生成新的自然语言推荐。" : `推荐失败：${error.message}`}</span>
        </div>
      )}

      {result && !error && (
        <>
          <div className="statusline">
            {result.ai_status === "used" && <span className="chip accent">AI 已增强</span>}
            {result.ai_status === "cached" && <span className="chip accent">AI 缓存命中</span>}
            {result.ai_status === "fallback" && (
              <span className="chip warn" title={result.fallback_reason ?? undefined}>
                确定性回退
              </span>
            )}
            {result.ai_status === "disabled" && (
              <span className="chip warn" title={result.fallback_reason ?? undefined}>
                AI 未启用
              </span>
            )}
            {result.interpreted.party_size !== null && <span className="chip">{result.interpreted.party_size} 人</span>}
            {result.interpreted.session_minutes_max !== null && <span className="chip">最长 {result.interpreted.session_minutes_max} 分钟</span>}
            {result.interpreted.coop_competitive !== null && (
              <span className="chip">{result.interpreted.coop_competitive < 0.5 ? "偏合作" : "偏竞技"}</span>
            )}
            {result.interpreted.self_hosting_willingness != null &&
              result.interpreted.self_hosting_willingness >= 0.5 && (
                <span className="chip">自建服优先</span>
              )}
            <span className={isStale(result.data_updated_at_ms) ? "chip warn" : "chip"}>
              数据更新于 {formatAgo(result.data_updated_at_ms)}
            </span>
          </div>
          {(result.ai_status === "fallback" || result.ai_status === "disabled") && (
            <p className="cal-note">
              {result.fallback_reason ??
                "当前由确定性规则理解输入；无法识别的条件不会被伪造成已理解。"}
            </p>
          )}
          {(result.ai_status === "used" || result.ai_status === "cached") && result.ai_summary && (
            <p className="cal-note">{result.ai_summary}</p>
          )}
          {result.items.length === 0 ? (
            <div className="state-box"><span className="big">∅</span><span>没有找到满足已识别条件的候选。</span></div>
          ) : (
            <div className="feed-grid">
              {result.items.map((item) => (
                <GameCard key={item.app_id} item={item} onOpen={onOpenGame} />
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}
