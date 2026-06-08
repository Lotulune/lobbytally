import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { GameAnalysisReport, GameCard, UserGameStatePatch } from "../../types";
import { getDisplayedGameScore } from "../../features/library/gameScoreDisplay";
import { GameAnalysisPanel } from "./GameAnalysisPanel";
import { useGameAnalysis } from "./useGameAnalysis";

type DetailTab = "ai" | "reviews" | "related";
const detailTabs: DetailTab[] = ["ai", "reviews", "related"];
const tabIds: Record<DetailTab, string> = {
  ai: "detail-tab-ai",
  reviews: "detail-tab-reviews",
  related: "detail-tab-related",
};
const panelIds: Record<DetailTab, string> = {
  ai: "detail-panel-ai",
  reviews: "detail-panel-reviews",
  related: "detail-panel-related",
};

export function DetailPage({
  game,
  relatedGames,
  onBack,
  onAnalysisUpdated,
  onToggleState,
  isBusy,
  analysisReadOnly = false,
}: {
  game: GameCard;
  relatedGames: GameCard[];
  onBack: () => void;
  onAnalysisUpdated?: (report: GameAnalysisReport) => Promise<void> | void;
  onToggleState: (patch: UserGameStatePatch, message: string) => void;
  isBusy: boolean;
  analysisReadOnly?: boolean;
}) {
  const [activeTab, setActiveTab] = useState<DetailTab>("ai");
  const [backQueued, setBackQueued] = useState(false);
  const { report, loading, error, expanded, refresh, toggleExpanded } = useGameAnalysis(
    game,
    onAnalysisUpdated,
    {
      missingMessage: "公共发现服务暂未公开这款游戏的分析结果。",
      readOnly: analysisReadOnly,
    },
  );
  const scoreDisplay = getDisplayedGameScore(game);
  const analysisRefreshPromiseRef = useRef<Promise<void> | null>(null);
  const backQueuedRef = useRef(false);
  const tabRefs = useRef<Record<DetailTab, HTMLButtonElement | null>>({
    ai: null,
    reviews: null,
    related: null,
  });
  const storeGalleryImages = [
    ...new Set(
      [...(game.storeScreenshotUrls?.length ? game.storeScreenshotUrls : [game.capsuleUrl])]
        .map((url) => url.trim())
        .filter((url) => url.length > 0),
    ),
  ].slice(0, 5);
  const primaryMediaUrl = storeGalleryImages[0] ?? game.capsuleUrl;
  const [activeMediaUrl, setActiveMediaUrl] = useState(primaryMediaUrl);
  const infoRows = [
    `多人模式：${joinOrFallback(game.multiplayerModes, 2)}`,
    `标签：${joinOrFallback(game.tags, 3)}`,
    `评论摘录：${game.reviewSnippets.length} 条`,
    `当前在线：${formatNumber(game.currentPlayers)}`,
    `${scoreDisplay.label}：${Math.round(scoreDisplay.value)}`,
  ];

  useEffect(() => {
    setActiveMediaUrl(primaryMediaUrl);
  }, [game.appid, primaryMediaUrl]);

  useEffect(() => {
    backQueuedRef.current = false;
    setBackQueued(false);
  }, [game.appid]);

  async function handleAnalysisRefresh() {
    const runRefresh = async () => {
      await refresh();
    };

    const pendingRefresh = runRefresh().finally(() => {
      analysisRefreshPromiseRef.current = null;
      if (backQueuedRef.current) {
        backQueuedRef.current = false;
        setBackQueued(false);
        onBack();
      }
    });

    analysisRefreshPromiseRef.current = pendingRefresh;
    await pendingRefresh;
  }

  function handleBack() {
    if (analysisRefreshPromiseRef.current) {
      backQueuedRef.current = true;
      setBackQueued(true);
      return;
    }

    onBack();
  }

  function focusTab(nextTab: DetailTab) {
    setActiveTab(nextTab);
    tabRefs.current[nextTab]?.focus();
  }

  function handleTabKeyDown(
    event: KeyboardEvent<HTMLButtonElement>,
    tab: DetailTab,
  ) {
    const currentIndex = detailTabs.indexOf(tab);
    let nextTab: DetailTab | null = null;

    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        nextTab = detailTabs[(currentIndex + 1) % detailTabs.length];
        break;
      case "ArrowLeft":
      case "ArrowUp":
        nextTab =
          detailTabs[(currentIndex - 1 + detailTabs.length) % detailTabs.length];
        break;
      case "Home":
        nextTab = detailTabs[0];
        break;
      case "End":
        nextTab = detailTabs[detailTabs.length - 1];
        break;
      default:
        return;
    }

    event.preventDefault();
    focusTab(nextTab);
  }

  return (
    <section className="detail-page">
      <div className="detail-toolbar">
        <button type="button" onClick={handleBack}>
          {backQueued ? "AI 评估完成后返回…" : "← 返回"}
        </button>
      </div>
      <div className="detail-grid">
        <div>
          <div className="hero-cover">
            <img src={activeMediaUrl} alt={`${game.name} 商店展示图`} />
            <div className="card-pill-stack">
              {game.isFree ? <span className="demo-pill free">Free</span> : null}
              <span>{demoLabel(game.demoStatus)}</span>
            </div>
          </div>
          <div className="thumb-row">
            {storeGalleryImages.map((imageUrl, index) => (
              <button
                key={imageUrl}
                aria-label={`查看《${game.name}》展示图 ${index + 1}`}
                aria-pressed={activeMediaUrl === imageUrl}
                className={activeMediaUrl === imageUrl ? "active" : ""}
                type="button"
                onClick={() => setActiveMediaUrl(imageUrl)}
              >
                <img src={imageUrl} alt="" />
              </button>
            ))}
          </div>

          <div aria-label="详情内容切换" className="detail-tabs" role="tablist">
            <button
              aria-controls={panelIds.ai}
              aria-selected={activeTab === "ai"}
              className={activeTab === "ai" ? "active" : ""}
              id={tabIds.ai}
              ref={(node) => {
                tabRefs.current.ai = node;
              }}
              role="tab"
              tabIndex={activeTab === "ai" ? 0 : -1}
              type="button"
              onKeyDown={(event) => handleTabKeyDown(event, "ai")}
              onClick={() => setActiveTab("ai")}
            >
              AI 评估
            </button>
            <button
              aria-controls={panelIds.reviews}
              aria-selected={activeTab === "reviews"}
              className={activeTab === "reviews" ? "active" : ""}
              id={tabIds.reviews}
              ref={(node) => {
                tabRefs.current.reviews = node;
              }}
              role="tab"
              tabIndex={activeTab === "reviews" ? 0 : -1}
              type="button"
              onKeyDown={(event) => handleTabKeyDown(event, "reviews")}
              onClick={() => setActiveTab("reviews")}
            >
              玩家评价 ({formatNumber(game.totalReviews)})
            </button>
            <button
              aria-controls={panelIds.related}
              aria-selected={activeTab === "related"}
              className={activeTab === "related" ? "active" : ""}
              id={tabIds.related}
              ref={(node) => {
                tabRefs.current.related = node;
              }}
              role="tab"
              tabIndex={activeTab === "related" ? 0 : -1}
              type="button"
              onKeyDown={(event) => handleTabKeyDown(event, "related")}
              onClick={() => setActiveTab("related")}
            >
              相关游戏 ({relatedGames.length})
            </button>
          </div>

          {activeTab === "ai" && (
            <div
              aria-labelledby={tabIds.ai}
              id={panelIds.ai}
              role="tabpanel"
            >
              <GameAnalysisPanel
                error={error}
                expanded={expanded}
                loading={loading}
                readOnly={analysisReadOnly}
                report={report}
                onRefresh={() => {
                  void handleAnalysisRefresh();
                }}
                onToggleExpanded={toggleExpanded}
              />
            </div>
          )}

          {activeTab === "reviews" && (
            <div
              aria-labelledby={tabIds.reviews}
              className="detail-content-panel"
              id={panelIds.reviews}
              role="tabpanel"
            >
              <h3>玩家评价摘录</h3>
              {game.reviewSnippets.length > 0 ? (
                <div className="review-snippet-list">
                  {game.reviewSnippets.map((snippet, index) => (
                    <article className="review-snippet-card" key={`${snippet.review}-${index}`}>
                      <div className="review-snippet-meta">
                        <strong
                          className={
                            snippet.votedUp
                              ? "review-sentiment review-sentiment-positive"
                              : "review-sentiment review-sentiment-negative"
                          }
                        >
                          {snippet.votedUp ? "✅ 推荐" : "❌ 不推荐"}
                        </strong>
                        <span>{formatHours(snippet.playtimeHours)}</span>
                      </div>
                      <p>{snippet.review}</p>
                    </article>
                  ))}
                </div>
              ) : (
                <div className="detail-empty-state">
                  <h3>还没有可展示的玩家评价</h3>
                  <p>当前数据源尚未返回评论摘录，之后同步到本地库后会显示在这里。</p>
                </div>
              )}
            </div>
          )}

          {activeTab === "related" && (
            <div
              aria-labelledby={tabIds.related}
              className="detail-content-panel"
              id={panelIds.related}
              role="tabpanel"
            >
              <h3>相关游戏</h3>
              {relatedGames.length > 0 ? (
                <div className="related-game-grid">
                  {relatedGames.map((relatedGame) => (
                    <article className="related-game-card" key={relatedGame.appid}>
                      <img src={relatedGame.capsuleUrl} alt="" />
                      <div>
                        <h4>{relatedGame.name}</h4>
                        <p>
                          {relatedGame.tags.slice(0, 3).join(" · ")} ·{" "}
                          {relatedGame.multiplayerModes.slice(0, 2).join(" · ")}
                        </p>
                        <span>
                          {formatPct(relatedGame.positiveReviewPct)} 好评 ·{" "}
                          {formatGameScore(relatedGame)}
                        </span>
                      </div>
                    </article>
                  ))}
                </div>
              ) : (
                <div className="detail-empty-state">
                  <h3>还没有相关游戏</h3>
                  <p>当前没有可关联的候选项，稍后同步更多游戏后会补全这一栏。</p>
                </div>
              )}
            </div>
          )}
        </div>

        <aside className="detail-side">
          <h2>{game.name}</h2>
          {game.shortDescription ? (
            <p className="detail-description">{game.shortDescription}</p>
          ) : null}
          <p>{game.tags.join(" · ")} · {game.multiplayerModes.slice(0, 2).join(" · ")}</p>
          <div className="detail-stat-grid">
            <span>♟ {formatPct(game.positiveReviewPct)} 好评</span>
            <span>♟ {formatNumber(game.currentPlayers)}</span>
            <span>发售于 {game.releaseDateText}</span>
            <span>{demoLabel(game.demoStatus)}</span>
            {game.isFree ? <span>Free To Play</span> : null}
          </div>
          <button
            className="gold-button"
            type="button"
            onClick={() =>
              onToggleState(
                { wishlist: !game.userState.wishlist },
                game.userState.wishlist
                  ? `已将《${game.name}》移出愿望单。`
                  : `已将《${game.name}》加入愿望单。`,
              )
            }
          >
            {game.userState.wishlist ? "已在愿望单" : "加入愿望单"}
          </button>
          <button
            className="muted-button"
            type="button"
            onClick={() =>
              onToggleState(
                { followed: !game.userState.followed },
                game.userState.followed
                  ? `已取消关注《${game.name}》。`
                  : `已关注《${game.name}》。`,
              )
            }
          >
            {game.userState.followed ? "已关注" : "关注"}
          </button>
          <button
            className="muted-button"
            type="button"
            onClick={() =>
              onToggleState(
                { favorite: !game.userState.favorite },
                game.userState.favorite
                  ? `已取消收藏《${game.name}》。`
                  : `已收藏《${game.name}》。`,
              )
            }
          >
            {game.userState.favorite ? "已收藏" : "收藏"}
          </button>

          <div className="info-box">
            <h3>数据摘要</h3>
            {infoRows.map((row) => (
              <p key={row}>♙ {row}</p>
            ))}
          </div>

          <div className="tag-panel compact">
            {game.tags.concat(game.multiplayerModes).slice(0, 8).map((tag) => (
              <em key={tag}>{tag}</em>
            ))}
          </div>

          {analysisReadOnly ? null : (
            <button
              className="gold-button"
              disabled={isBusy || loading}
              type="button"
              onClick={() => {
                void handleAnalysisRefresh();
              }}
            >
              {isBusy || loading ? "AI 评估中…" : "重新 AI 评估"}
            </button>
          )}
        </aside>
      </div>
    </section>
  );
}

function demoLabel(status: GameCard["demoStatus"]) {
  switch (status) {
    case "demo_only":
      return "Demo";
    case "released_with_demo":
      return "Demo & 已发售";
    case "released":
      return "已发售";
    case "unknown":
      return "未知";
  }
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}

function formatGameScore(game: Pick<GameCard, "aiScore" | "recommendationScore">) {
  const scoreDisplay = getDisplayedGameScore(game);
  return `${scoreDisplay.label} ${Math.round(scoreDisplay.value)}`;
}

function formatNumber(value?: number | null) {
  return typeof value === "number" ? value.toLocaleString("zh-CN") : "—";
}

function formatHours(value?: number | null) {
  return typeof value === "number" ? `${value} 小时游玩` : "游玩时长未知";
}

function joinOrFallback(items: string[], limit: number) {
  const visibleItems = items.slice(0, limit);
  return visibleItems.length > 0 ? visibleItems.join(" · ") : "待补充";
}
