import type { GameAnalysisReport } from "../../types";

export function GameAnalysisPanel({
  report,
  loading,
  error,
  expanded,
  readOnly = false,
  onRefresh,
  onToggleExpanded,
}: {
  report: GameAnalysisReport | null;
  loading: boolean;
  error: string | null;
  expanded: boolean;
  readOnly?: boolean;
  onRefresh: () => void;
  onToggleExpanded: () => void;
}) {
  if (!report) {
    if (loading) {
      return (
        <div className="detail-content-panel ai-analysis-report ai-analysis-state">
          <h3>AI 详细评估</h3>
          <p>
            {readOnly
              ? "正在读取公共发现服务公开的分析结果，请稍等片刻。"
              : "正在生成这款游戏的详细分析，请稍等片刻。"}
          </p>
        </div>
      );
    }

    if (error) {
      return (
        <div className="detail-content-panel ai-analysis-report ai-analysis-state">
          <h3>AI 详细评估</h3>
          <p>{error}</p>
          {readOnly ? null : (
            <button className="gold-button" type="button" onClick={onRefresh}>
              重试生成
            </button>
          )}
        </div>
      );
    }

    return (
      <div className="detail-content-panel ai-analysis-report ai-analysis-state">
        <h3>AI 详细评估</h3>
        <p>当前还没有可展示的分析结果。</p>
      </div>
    );
  }

  const expandedRegionId = `game-analysis-expanded-${report.appid}`;
  const recommendationScore =
    typeof report.recommendationScore === "number"
      ? report.recommendationScore
      : report.overallScore;
  const qualityScore =
    typeof report.qualityScore === "number"
      ? report.qualityScore
      : report.overallScore;

  return (
    <div className="detail-content-panel ai-analysis-report">
      <div className="analysis-summary-card">
        <div className="analysis-summary-head">
          <div className="analysis-summary-copy">
            <h3>AI 详细评估</h3>
            <p>{report.overview}</p>
          </div>
          <div className="analysis-score-card">
            <strong>{Math.round(recommendationScore)}</strong>
            <span>综合推荐</span>
          </div>
          <div className="analysis-score-card">
            <strong>{Math.round(qualityScore)}</strong>
            <span>游戏质量</span>
          </div>
        </div>

        <div className="analysis-summary-badges">
          <span className="analysis-badge analysis-badge-source">
            {formatSource(report.source)}
          </span>
          <span className="analysis-badge analysis-badge-confidence">
            {formatConfidence(report.confidence)}
          </span>
          {report.poolType ? (
            <span className="analysis-badge">{formatPoolType(report.poolType)}</span>
          ) : null}
          <span className="analysis-badge">更新于 {formatGeneratedAt(report.generatedAt)}</span>
        </div>

        {report.riskFlags?.length ? (
          <div className="analysis-summary-badges">
            {report.riskFlags.slice(0, 3).map((flag) => (
              <span className="analysis-badge" key={flag.key}>
                风险：{flag.label}
              </span>
            ))}
          </div>
        ) : null}

        {error ? <p className="analysis-inline-alert">刷新失败：{error}</p> : null}

        <div className="analysis-dimension-grid">
          {report.dimensionScores.map((dimension) => (
            <article className="analysis-dimension-card" key={dimension.key}>
              <div className="analysis-dimension-head">
                <span>{dimension.label}</span>
                <strong>{Math.round(dimension.score)}</strong>
              </div>
              <i className="analysis-dimension-track">
                <b style={{ width: `${Math.round(dimension.score)}%` }} />
              </i>
              <p>{dimension.reason}</p>
            </article>
          ))}
        </div>

        <div className="analysis-action-row">
          <button
            aria-controls={expandedRegionId}
            aria-expanded={expanded}
            className="muted-button"
            type="button"
            onClick={onToggleExpanded}
          >
            {expanded ? "收起完整报告" : "查看完整报告"}
          </button>
          {readOnly ? null : (
            <button className="muted-button" disabled={loading} type="button" onClick={onRefresh}>
              {loading ? "刷新中…" : "刷新分析"}
            </button>
          )}
          <span className="analysis-action-hint">
            {readOnly
              ? "公共服务分析为只读结果，可展开查看优势、风险与证据明细。"
              : loading
                ? "正在更新分析结果…"
                : "可展开查看优势、风险与证据明细。"}
          </span>
        </div>
      </div>

      <div
        aria-label="完整分析报告"
        className="analysis-expanded-grid"
        hidden={!expanded}
        id={expandedRegionId}
        role="region"
      >
          <section>
            <h4>优势亮点</h4>
            <div className="analysis-point-list">
              {report.strengths.length > 0 ? (
                report.strengths.map((point, index) => (
                  <article className="analysis-point-card" key={`${point.title}-${index}`}>
                    <strong>{point.title}</strong>
                    <p>{point.reason}</p>
                  </article>
                ))
              ) : (
                <p className="analysis-empty-copy">当前没有提炼出明显优势。</p>
              )}
            </div>
          </section>

          <section>
            <h4>关注风险</h4>
            <div className="analysis-point-list">
              {report.risks.length > 0 ? (
                report.risks.map((point, index) => (
                  <article className="analysis-point-card" key={`${point.title}-${index}`}>
                    <strong>{point.title}</strong>
                    <p>{point.reason}</p>
                  </article>
                ))
              ) : (
                <p className="analysis-empty-copy">当前没有提炼出明显风险。</p>
              )}
            </div>
          </section>

          <section>
            <h4>结构化证据</h4>
            <div className="analysis-evidence-list">
              {report.evidence.length > 0 ? (
                report.evidence.map((item, index) => (
                  <article className="analysis-evidence-card" key={`${item.label}-${index}`}>
                    <div className="analysis-evidence-head">
                      <strong>{item.label}</strong>
                      <span>{item.value}</span>
                    </div>
                    <p>{item.interpretation}</p>
                  </article>
                ))
              ) : (
                <p className="analysis-empty-copy">当前没有结构化证据。</p>
              )}
            </div>
          </section>

          <section>
            <h4>玩家评价证据</h4>
            <div className="analysis-review-list">
              {report.reviewEvidence.length > 0 ? (
                report.reviewEvidence.map((item, index) => (
                  <article className="analysis-review-card" key={`${item.quote}-${index}`}>
                    <div className="analysis-review-head">
                      <strong
                        className={
                          item.stance === "strength"
                            ? "review-sentiment review-sentiment-positive"
                            : "review-sentiment review-sentiment-negative"
                        }
                      >
                        {item.stance === "strength" ? "正向证据" : "风险证据"}
                      </strong>
                      <span>{item.playtimeText}</span>
                    </div>
                    <blockquote>{formatReviewQuote(item.quote)}</blockquote>
                    <p>{item.interpretation}</p>
                  </article>
                ))
              ) : (
                <p className="analysis-empty-copy">当前没有可引用的评论证据。</p>
              )}
            </div>
          </section>
      </div>
    </div>
  );
}

function formatReviewQuote(value: string) {
  return value
    .replace(/\[url=[^\]]+\]([\s\S]*?)\[\/url\]/gi, "$1")
    .replace(/\[(?:\/)?[a-z*]+(?:=[^\]]+)?\]/gi, "")
    .replace(/\s+\n/g, "\n")
    .replace(/\n\s+/g, "\n")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
}

function formatSource(source: GameAnalysisReport["source"]) {
  return source === "hybrid" ? "混合分析" : "规则分析";
}

function formatConfidence(confidence: GameAnalysisReport["confidence"]) {
  switch (confidence) {
    case "high":
      return "高置信";
    case "medium":
      return "中置信";
    case "low":
      return "低置信";
  }
}

function formatGeneratedAt(generatedAt: string) {
  const date = new Date(generatedAt);
  if (Number.isNaN(date.getTime())) {
    return generatedAt;
  }

  return date.toLocaleString("zh-CN", {
    hour12: false,
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatPoolType(poolType: NonNullable<GameAnalysisReport["poolType"]>) {
  switch (poolType) {
    case "new_release":
      return "新游候选池";
    case "evergreen":
      return "稳定推荐池";
    case "hidden_gem":
      return "冷门宝藏池";
    case "friends_party":
      return "朋友开黑池";
    case "demo_potential":
      return "Demo 潜力池";
  }
}
