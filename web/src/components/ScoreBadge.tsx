// Context-relative recommendation badge (.score-badge).

import { recommendationLabel } from "../app/format";

export function ScoreBadge({
  rank,
  recommendationIndex,
  dataConfidence,
  fitBand,
}: {
  rank: number | null | undefined;
  recommendationIndex: number | null | undefined;
  dataConfidence: number | null | undefined;
  fitBand?: string | null;
}) {
  return (
    <span
      className="score-badge"
      title="当前推荐列表中的相对位置，不代表喜欢或购买概率"
    >
      {recommendationLabel(rank, recommendationIndex, dataConfidence, fitBand)}
    </span>
  );
}
