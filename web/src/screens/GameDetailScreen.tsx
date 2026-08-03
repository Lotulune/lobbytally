// Game detail: hero (cover/title/status/想玩/Steam link), info panels
// (multiplayer profile, availability, reviews/CCU, evidence) and the
// expandable popular-review wall. Page-specific styles live in
// styles/screens/game-detail.css, scoped under .detail-screen.

import { useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type { EvidenceItem, GameDetail, PopularReview } from "../api/types";
import {
  dominantModeLabel,
  evidenceValueLabel,
  featureLabel,
  formatAgo,
  formatReleaseDate,
  formatCount,
  formatPercent,
  formatPrice,
  isStale,
  languageLabels,
  partyLabel,
  platformLabels,
  positiveRate,
  releaseStateLabel,
  sourceTypeLabel,
} from "../app/format";
import { apiClient } from "../app/runtime";
import { useTheme } from "../app/ThemeProvider";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { EmptyState } from "../components/EmptyState";
import { GameMediaGallery } from "../components/GameMediaGallery";
import { Panel } from "../components/Panel";
import { Skeleton } from "../components/Skeleton";
import { VoteButton } from "../components/VoteButton";

function boolLabel(value: boolean | null): string {
  if (value === true) return "支持";
  if (value === false) return "不支持";
  return "未知";
}

/** At-a-glance capability chip in the hero. Unknown stays neutral, never red. */
function CapabilityChip({ label, value }: { label: string; value: boolean | null }) {
  const tone = value === true ? "ok" : value === false ? "warn" : undefined;
  return (
    <Chip tone={tone} title={value === null ? "暂无证据，尚未确认" : undefined}>
      {label}：{boolLabel(value)}
    </Chip>
  );
}

function fallbackSummary(game: GameDetail): string {
  const mp = game.multiplayer;
  const capabilities = [
    mp.private_session === true ? "可创建私人房间" : null,
    mp.online_coop === true ? "支持在线合作" : null,
    mp.self_hosted_server === true ? "可自建服务器" : null,
  ].filter((value): value is string => value !== null);
  const party = partyLabel(mp.recommended_min, mp.recommended_max);
  const capabilityText = capabilities.length > 0 ? capabilities.join("，") : "联机能力仍待补充";
  const partyText =
    party === "人数未定" ? "推荐人数仍待补充" : `推荐 ${party}`;
  return `${dominantModeLabel(mp.dominant_mode)}玩法，${partyText}；${capabilityText}。`;
}

function reviewDate(timestampMs: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(new Date(timestampMs));
}

function playtimeLabel(minutes: number | null): string | null {
  if (minutes === null) return null;
  const hours = minutes / 60;
  return `游玩 ${hours >= 100 ? Math.round(hours) : hours.toFixed(1)} 小时`;
}

function PopularReviewCard({ review }: { review: PopularReview }) {
  const [expanded, setExpanded] = useState(false);
  const isLong = review.text.length > 360;
  const playtime = playtimeLabel(review.playtime_forever_minutes);
  return (
    <article className="steam-review-card">
      <div className="steam-review-head">
        <Chip tone={review.voted_up ? "ok" : "danger"}>
          {review.voted_up ? "👍 推荐" : "👎 不推荐"}
        </Chip>
        {review.author_profile_url ? (
          <a href={review.author_profile_url} target="_blank" rel="noreferrer noopener">
            {review.author_name || "Steam 玩家"} ↗
          </a>
        ) : (
          <strong>{review.author_name || "Steam 玩家"}</strong>
        )}
        <span className="steam-review-rank">热门 #{review.rank}</span>
      </div>
      <div className="steam-review-meta">
        <span>{reviewDate(review.created_at_ms)}</span>
        {playtime && <span>{playtime}</span>}
        <span>{formatCount(review.votes_up)} 人觉得有用</span>
        {review.votes_funny > 0 && <span>{formatCount(review.votes_funny)} 人觉得欢乐</span>}
        {review.comment_count > 0 && <span>{formatCount(review.comment_count)} 条评论</span>}
        {review.written_during_early_access && <span>抢先体验期间撰写</span>}
      </div>
      <p className={expanded ? "steam-review-text expanded" : "steam-review-text"}>
        {review.text}
      </p>
      {isLong && (
        <button type="button" className="review-expand" onClick={() => setExpanded((value) => !value)}>
          {expanded ? "收起" : "展开全文"}
        </button>
      )}
    </article>
  );
}

interface DetailState {
  detail: GameDetail | null;
  evidence: EvidenceItem[];
  loading: boolean;
  error: ApiError | null;
  fromOfflineCache: boolean;
}

export function GameDetailScreen({
  appId,
  onBack,
  recommendationRunId = null,
}: {
  appId: number;
  onBack: () => void;
  recommendationRunId?: string | null;
}) {
  const [state, setState] = useState<DetailState>({
    detail: null,
    evidence: [],
    loading: true,
    error: null,
    fromOfflineCache: false,
  });
  const { fireAction } = useTheme();
  const steamBtnRef = useRef<HTMLAnchorElement>(null);

  useEffect(() => {
    let cancelled = false;
    setState((prev) => ({ ...prev, loading: true, error: null }));
    Promise.all([apiClient.game(appId), apiClient.evidence(appId).catch(() => null)])
      .then(([game, evidence]) => {
        if (cancelled) return;
        setState({
          detail: game.data,
          evidence: evidence?.data.items ?? [],
          loading: false,
          error: null,
          fromOfflineCache: game.fromOfflineCache,
        });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setState({
          detail: null,
          evidence: [],
          loading: false,
          error:
            error instanceof ApiError
              ? error
              : new ApiError({ code: "unknown", status: 0, message: String(error) }),
          fromOfflineCache: false,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [appId]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      // Modal owns Escape while open (stops propagation); still guard for other dialogs.
      if (document.querySelector("[role='dialog'][aria-modal='true']")) return;
      onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  if (state.loading) {
    return (
      <div className="detail-screen" aria-busy="true">
        <div className="backbar">
          <Button size="small" onClick={onBack}>
            ← 返回 (Esc)
          </Button>
        </div>
        <div className="detail-hero">
          <div className="detail-cover">
            <div className="gallery-stage gallery-stage-skeleton" aria-hidden="true">
              <Skeleton height={225} />
            </div>
          </div>
          <div className="detail-hero-body">
            <Skeleton height={34} />
            <Skeleton height={26} />
            <Skeleton height={90} />
          </div>
        </div>
        <div className="detail-grid">
          <Skeleton height={200} />
          <Skeleton height={200} />
        </div>
      </div>
    );
  }

  if (state.error || !state.detail) {
    return (
      <div className="detail-screen">
        <div className="backbar">
          <Button size="small" onClick={onBack}>
            ← 返回 (Esc)
          </Button>
        </div>
        <EmptyState glyph="!" alert>
          <span>
            {state.error?.code === "not_found"
              ? "没有找到这个游戏。"
              : `详情加载失败：${state.error?.message ?? "未知错误"}`}
          </span>
        </EmptyState>
      </div>
    );
  }

  const game = state.detail;
  const mp = game.multiplayer;
  const av = game.availability;
  const partySizeLabel = partyLabel(mp.recommended_min, mp.recommended_max);
  const shortDescription = game.short_description?.trim() || null;
  const price = av.is_free === true || (av.final_price_minor !== null && av.price_currency)
    ? formatPrice(av.final_price_minor, av.price_currency, av.is_free)
    : "待同步 Steam 国区价格";

  return (
    <div className="detail-screen">
      <div className="backbar">
        <Button size="small" onClick={onBack}>
          ← 返回 (Esc)
        </Button>
        {state.fromOfflineCache && <Chip tone="danger">离线快照</Chip>}
        <Chip tone={isStale(game.data_updated_at_ms) ? "warn" : undefined}>
          数据更新于 {formatAgo(game.data_updated_at_ms)}
        </Chip>
      </div>

      <header className="detail-hero">
        <div className="detail-cover">
          <GameMediaGallery
            appId={game.app_id}
            name={game.name}
            coverUrl={game.cover_url}
            media={game.media}
            steamUrl={game.steam_url}
          />
        </div>
        <div className="detail-hero-body">
          <h2 className="detail-title">{game.name}</h2>
          <div className="detail-tags">
            <Chip tone="accent">{dominantModeLabel(mp.dominant_mode)}</Chip>
            <Chip>{releaseStateLabel(game.release_state)}</Chip>
            <Chip>{formatReleaseDate(game.release_date, game.release_date_raw, game.release_date_precision)}</Chip>
            {av.has_demo && <Chip tone="ok">有 Demo</Chip>}
          </div>
          <div className="detail-actions">
            <VoteButton
              appId={game.app_id}
              intent={game.play_intent}
              size="large"
              recommendationRunId={recommendationRunId}
            />
            <a
              ref={steamBtnRef}
              className="btn primary"
              href={game.steam_url}
              target="_blank"
              rel="noreferrer noopener"
              onClick={() => {
                if (recommendationRunId) {
                  void apiClient
                    .postRecommendationEvent({
                      recommendationRunId,
                      appId: game.app_id,
                      eventType: "steam_click",
                    })
                    .catch(() => undefined);
                }
                fireAction("confirm", steamBtnRef.current);
              }}
            >
              在 Steam 打开 ↗
            </a>
          </div>
          <div className="detail-capabilities" aria-label="联机能力速览">
            <CapabilityChip label="私人房间" value={mp.private_session} />
            <CapabilityChip label="在线合作" value={mp.online_coop} />
            <CapabilityChip label="自建服务器" value={mp.self_hosted_server} />
            <Chip
              title={
                partySizeLabel === "人数未定"
                  ? "商店分类仅能确认多人，无法可靠得到小队人数区间"
                  : undefined
              }
            >
              推荐人数：{partySizeLabel}
            </Chip>
          </div>
          <section className="detail-summary" aria-label={shortDescription ? "商店简介" : "联机速览"}>
            <h3>{shortDescription ? "商店简介" : "联机速览"}</h3>
            <p>{shortDescription ?? fallbackSummary(game)}</p>
            <span>{shortDescription ? "来源：Steam 商店" : "来源：已入库联机资料"}</span>
          </section>
        </div>
      </header>

      <div className="detail-grid">
        <Panel as="section" title="联机方式">
          <dl className="kv">
            <dt>推荐人数</dt>
            <dd>
              {partySizeLabel}
              {partySizeLabel === "人数未定" && (
                <Chip
                  tone="warn"
                  title="商店分类仅能确认多人，无法可靠得到小队人数区间"
                >
                  仅分类弱信号
                </Chip>
              )}
            </dd>
            <dt>私人房间</dt>
            <dd>{boolLabel(mp.private_session)}</dd>
            <dt>在线合作</dt>
            <dd>{boolLabel(mp.online_coop)}</dd>
            <dt>自建服务器</dt>
            <dd>{boolLabel(mp.self_hosted_server)}</dd>
            <dt>画像置信度</dt>
            <dd>
              {formatPercent(mp.profile_confidence)}
              {mp.profile_confidence !== null && mp.profile_confidence < 0.5 && (
                <Chip tone="warn">低置信</Chip>
              )}
            </dd>
          </dl>
        </Panel>

        <Panel as="section" title="可用性">
          <dl className="kv">
            <dt>平台</dt>
            <dd>{av.platforms.length > 0 ? platformLabels(av.platforms) : "待同步 Steam 商店资料"}</dd>
            <dt>语言</dt>
            <dd>{av.languages.length > 0 ? languageLabels(av.languages) : "待同步 Steam 商店资料"}</dd>
            <dt>单局时长</dt>
            <dd>
              {av.typical_session_minutes_min !== null && av.typical_session_minutes_max !== null
                ? `${av.typical_session_minutes_min}–${av.typical_session_minutes_max} 分钟`
                : "尚未录入"}
            </dd>
            <dt>价格</dt>
            <dd>{price}</dd>
          </dl>
        </Panel>

        <Panel as="section" title="评价与活跃度">
          <dl className="kv">
            <dt>累计评价</dt>
            <dd>{formatCount(game.reviews.total)}</dd>
            <dt>好评率</dt>
            <dd>{positiveRate(game.reviews.total, game.reviews.positive)}</dd>
            <dt>当前在线</dt>
            <dd>{game.latest_ccu !== null ? formatCount(game.latest_ccu) : "未知"}</dd>
          </dl>
        </Panel>

        <Panel as="section" title="特征证据" className="evidence-panel">
          {state.evidence.length === 0 ? (
            <span className="evidence-empty">暂无公开证据记录。</span>
          ) : (
            <>
              <div className="evidence-grid">
                {state.evidence.slice(0, 8).map((item) => (
                  <div key={item.evidence_id} className="evidence-item">
                    <span>
                      {featureLabel(item.feature)} = {evidenceValueLabel(item.value)}
                    </span>
                    <span className="src">
                      {sourceTypeLabel(item.source_type)} · 置信 {formatPercent(item.confidence)} ·{" "}
                      {formatAgo(item.observed_at_ms)}
                    </span>
                  </div>
                ))}
              </div>
              {state.evidence.length > 8 && (
                <span className="evidence-more">
                  仅显示前 8 条，共 {state.evidence.length} 条公开证据。
                </span>
              )}
            </>
          )}
        </Panel>

        <Panel as="section" className="review-panel">
          <div className="review-panel-title">
            <div>
              <h4>Steam 热门评价</h4>
              <span>简体中文 · 按 Steam 热门顺序 · 最多 10 条</span>
            </div>
          </div>
          {game.reviews.featured.length === 0 ? (
            <div className="review-empty">热门评价正文尚未同步。</div>
          ) : (
            <div className="steam-review-grid">
              {game.reviews.featured.map((review) => (
                <PopularReviewCard key={review.recommendation_id} review={review} />
              ))}
            </div>
          )}
        </Panel>
      </div>
    </div>
  );
}
