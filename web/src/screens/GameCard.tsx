// One recommendation card: large cover, release/review stats, reasons, feedback.
// Information priority is fixed: cover > title+score > meta chips > want-to-play
// > reasons > cautions > feedback actions.

import { useEffect, useRef, useState } from "react";
import type { FeedItem, FeedbackType } from "../api/types";
import { requestAccountSignIn } from "../app/auth";
import { apiClient, feedbackQueue } from "../app/runtime";
import { useTheme } from "../app/ThemeProvider";
import { useToast } from "../app/ToastProvider";
import {
  dominantModeLabel,
  FEEDBACK_LABELS,
  formatCount,
  formatReleaseDate,
  hasLowRecommendationConfidence,
  hasConcretePartySize,
  partyLabel,
  positiveRate,
  recommendationDataReliabilityLabel,
  recommendationSlotReasonLabel,
} from "../app/format";
import type { ActiveFeedbackDimensions, PendingFeedback } from "../api/feedbackQueue";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { ScoreBadge } from "../components/ScoreBadge";
import { VoteButton } from "../components/VoteButton";
import { GameMedia } from "../components/GameMedia";

const REASON_ACTIONS: { type: FeedbackType; label: string }[] = [
  { type: "party_size_mismatch", label: "人数不合适" },
  { type: "too_competitive", label: "竞技性不合适" },
  { type: "hosting_friction", label: "开服或匹配麻烦" },
];

export function GameCard({
  item,
  onOpen,
  recommendationRunId = null,
}: {
  item: FeedItem;
  onOpen: (appId: number, recommendationRunId?: string | null) => void;
  recommendationRunId?: string | null;
}) {
  const { fireAction } = useTheme();
  const toast = useToast();
  const cardRef = useRef<HTMLElement>(null);
  const [active, setActive] = useState<ActiveFeedbackDimensions>(
    () => feedbackQueue.activeDimensionsForApp(item.app_id),
  );
  const [showReasons, setShowReasons] = useState(false);

  useEffect(() => {
    return feedbackQueue.subscribe(() => {
      setActive(feedbackQueue.activeDimensionsForApp(item.app_id));
    });
  }, [item.app_id]);

  useEffect(() => {
    if (!recommendationRunId) return;
    void apiClient
      .postRecommendationEvent({
        recommendationRunId,
        appId: item.app_id,
        eventType: "exposure",
        idempotencyKey: `exposure:${item.app_id}`,
      })
      .catch(() => undefined);
  }, [item.app_id, recommendationRunId]);

  const openRecommendation = () => {
    if (recommendationRunId) {
      void apiClient
        .postRecommendationEvent({
          recommendationRunId,
          appId: item.app_id,
          eventType: "detail_open",
        })
        .catch(() => undefined);
    }
    onOpen(item.app_id, recommendationRunId);
  };

  const activeEntry = (type: FeedbackType): PendingFeedback | null => {
    if (type === "like" || type === "not_interested") {
      return active.sentiment?.type === type ? active.sentiment : null;
    }
    if (type === "played") return active.ownership;
    return active.reasons.find((entry) => entry.type === type) ?? null;
  };

  const undo = (entry: PendingFeedback) => {
    void feedbackQueue.undo(entry.localId).catch(() => {
      toast.show("撤销失败，请稍后再试");
    });
  };

  const toggleFeedback = (type: FeedbackType, target: Element | null) => {
    if (!apiClient.isAccountAuthenticated()) {
      requestAccountSignIn();
      return;
    }
    const existing = activeEntry(type);
    if (existing) {
      fireAction("dismiss", target);
      undo(existing);
      return;
    }
    const entry = feedbackQueue.submit(item.app_id, type, recommendationRunId);
    fireAction(type === "like" ? "like" : type === "not_interested" ? "dismiss" : "confirm", target);
    toast.show(`已记录「${FEEDBACK_LABELS[type] ?? type}」`, {
      label: "撤销",
      run: () => undo(entry),
    });
  };

  const ccu = item.typical_ccu_7d ?? item.latest_ccu;
  const releaseLabel = formatReleaseDate(
    item.release_date,
    item.release_date_raw,
    item.release_date_precision,
  );
  const hasReviews = typeof item.total_reviews === "number" && item.total_reviews > 0;
  const reviewLabel = hasReviews
    ? `${positiveRate(item.total_reviews, item.total_positive ?? null)} · ${formatCount(item.total_reviews)} 评`
    : null;
  const hasCcu = typeof ccu === "number" && ccu > 0;
  const mode = item.multiplayer?.dominant_mode ?? null;
  const partyMin = item.party?.recommended_min ?? null;
  const partyMax = item.party?.recommended_max ?? null;
  const showParty = hasConcretePartySize(partyMin, partyMax);
  const slotReasonLabel = recommendationSlotReasonLabel(item.slot_reason);
  const reasonsVisible =
    showReasons || active.sentiment?.type === "not_interested" || active.reasons.length > 0;
  const hasPendingFeedback = [active.sentiment, active.ownership, ...active.reasons]
    .filter((entry): entry is PendingFeedback => entry !== null)
    .some((entry) => entry.feedbackId === null || entry.syncError !== null);

  return (
    <article
      ref={cardRef}
      className="card card-with-cover"
      tabIndex={0}
      role="button"
      data-app-id={item.app_id}
      aria-label={`查看 ${item.name} 详情`}
      onClick={openRecommendation}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          openRecommendation();
        }
      }}
    >
      <GameMedia coverUrl={item.cover_url} name={item.name} appId={item.app_id} />
      <div className="card-body">
        <div className="card-title">
          <h3>{item.name}</h3>
          <ScoreBadge
            rank={item.rank}
            recommendationIndex={item.recommendation_index}
            dataConfidence={item.data_confidence}
            fitBand={item.fit_band}
          />
        </div>
        <div className="card-meta">
          {slotReasonLabel && <Chip>{slotReasonLabel}</Chip>}
          <Chip tone="accent">{dominantModeLabel(mode)}</Chip>
          {showParty && <Chip>{partyLabel(partyMin, partyMax)}</Chip>}
          {releaseLabel !== "日期未定" && <Chip>{releaseLabel}</Chip>}
          {reviewLabel && <Chip>{reviewLabel}</Chip>}
          {hasCcu && <Chip>约 {formatCount(ccu)} 在线</Chip>}
          <Chip tone={hasLowRecommendationConfidence(item.data_confidence) ? "warn" : undefined}>
            {recommendationDataReliabilityLabel(item.data_confidence)}
          </Chip>
        </div>
        <div
          className="card-vote"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
        >
          <VoteButton
            appId={item.app_id}
            intent={item.play_intent}
            recommendationRunId={recommendationRunId}
          />
          <span className="card-vote-hint">全站玩家想玩人数会小幅影响推荐</span>
        </div>
        {item.ai_reasons && item.ai_reasons.length > 0 && (
          <div className="reason-block">
            <span className="reason-tag">AI 分析</span>
            <ul className="reason-list">
              {item.ai_reasons.slice(0, 3).map((reason) => (
                <li key={`ai-${reason}`}>{reason}</li>
              ))}
            </ul>
          </div>
        )}
        {item.reasons && item.reasons.length > 0 ? (
          <ul className="reason-list">
            {item.reasons.slice(0, 3).map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        ) : (
          <p className="card-empty-hint">
            {[
              releaseLabel !== "日期未定" ? `发售 ${releaseLabel}` : null,
              reviewLabel,
              hasCcu ? `约 ${formatCount(ccu)} 在线` : null,
              mode ? dominantModeLabel(mode) : "联机画像未校准",
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        )}
        {item.cautions && item.cautions.length > 0 && (
          <ul className="caution-list">
            {item.cautions.slice(0, 2).map((caution) => (
              <li key={caution}>{caution}</li>
            ))}
          </ul>
        )}
        <div
          className="card-actions"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
        >
          <div className="feedback-control-row">
            <span className="feedback-control-label">感受</span>
            <div className="seg" role="group" aria-label={`${item.name} 的喜欢程度`}>
              <Button
                size="small"
                aria-pressed={active.sentiment?.type === "like"}
                onClick={(event) => {
                  toggleFeedback("like", event.currentTarget);
                }}
              >
                喜欢
              </Button>
              <Button
                size="small"
                aria-pressed={active.sentiment?.type === "not_interested"}
                onClick={(event) => {
                  setShowReasons(true);
                  toggleFeedback("not_interested", event.currentTarget);
                }}
              >
                不感兴趣
              </Button>
            </div>
          </div>
          <div className="feedback-control-row">
            <span className="feedback-control-label">状态</span>
            <Button
              size="small"
              aria-pressed={active.ownership?.type === "played"}
              onClick={(event) => toggleFeedback("played", event.currentTarget)}
            >
              玩过 / 已拥有
            </Button>
            <span className="feedback-control-note">“想玩”在上方单独记录</span>
          </div>
          {reasonsVisible && (
            <div className="feedback-control-row feedback-reasons">
              <span className="feedback-control-label">原因（可多选）</span>
              <div className="feedback-reason-list" role="group" aria-label="不感兴趣的原因">
                {REASON_ACTIONS.map((reason) => (
                  <Button
                    key={reason.type}
                    size="small"
                    aria-pressed={active.reasons.some((entry) => entry.type === reason.type)}
                    onClick={(event) => toggleFeedback(reason.type, event.currentTarget)}
                  >
                    {reason.label}
                  </Button>
                ))}
              </div>
            </div>
          )}
          {hasPendingFeedback && <Chip tone="warn">反馈待同步</Chip>}
        </div>
      </div>
    </article>
  );
}
