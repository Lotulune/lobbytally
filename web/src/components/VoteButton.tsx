// Community "want to play" vote control. Optimistic via the play-intent store;
// server responses (feed / authed detail) supply the authoritative baseline.

import { useEffect, useState } from "react";
import type { PlayIntentSummary } from "../api/types";
import { requestAccountSignIn } from "../app/auth";
import { apiClient, playIntentStore } from "../app/runtime";
import { useTheme } from "../app/ThemeProvider";

export function VoteButton({
  appId,
  intent,
  size = "small",
  recommendationRunId = null,
}: {
  appId: number;
  /** Optional: cached responses from before this feature may lack the field. */
  intent?: PlayIntentSummary;
  size?: "small" | "large";
  recommendationRunId?: string | null;
}) {
  const { fireAction } = useTheme();
  const [, force] = useState(0);

  useEffect(() => playIntentStore.subscribe(() => force((n) => n + 1)), []);

  const base = intent ?? { count: 0, voted: false };
  const voted = playIntentStore.effectiveVoted(appId, base.voted);
  const count = Math.max(0, base.count + playIntentStore.countDelta(appId, base.voted));
  const pending = playIntentStore.isPending(appId);

  return (
    <button
      type="button"
      className={`vote-btn${voted ? " voted" : ""}${size === "large" ? " large" : ""}`}
      aria-pressed={voted}
      aria-label={voted ? `取消想玩，共 ${count} 人想玩` : `标记想玩，共 ${count} 人想玩`}
      title="全站票数达到最低样本后，仅小幅影响推荐"
      onClick={(event) => {
        event.stopPropagation();
        if (!apiClient.isAccountAuthenticated()) {
          requestAccountSignIn();
          return;
        }
        playIntentStore.toggle(appId, base.voted, recommendationRunId);
        fireAction(voted ? "dismiss" : "like", event.currentTarget);
      }}
    >
      <span className="vote-glyph" aria-hidden="true">
        ▲
      </span>
      <span className="vote-label">想玩</span>
      <span className="vote-count">{count}</span>
      {pending && <span className="vote-pending" aria-hidden="true" />}
    </button>
  );
}
