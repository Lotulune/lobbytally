import type { GameCard } from "../../types";
import { CollectionGrid } from "./CollectionGrid";

export function HistoryPage({
  games,
  onOpen,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
}) {
  const sortedGames = [...games].sort(compareByUpdatedAtDesc);

  return (
    <section className="favorites-page">
      <div className="collection-page-head">
        <div>
          <h2>游玩记录</h2>
          <p>这里展示你最近打开过详情页的游戏，按本地 userState.updatedAt 从新到旧排序。</p>
        </div>
      </div>
      <CollectionGrid
        countLabel={`最近浏览 · ${sortedGames.length} 款`}
        emptyBody="打开过详情页的游戏会按最近浏览时间排列在这里，方便回看最近关注过什么。"
        emptyTitle="最近还没有浏览记录"
        games={sortedGames}
        onOpen={onOpen}
        renderBadge={(game) => (
          <span className="favorite-history-stamp">{formatUpdatedAt(game.userState.updatedAt)}</span>
        )}
        renderMeta={(game) => ({
          primary: formatUpdatedAt(game.userState.updatedAt),
          secondary: game.multiplayerModes[0] ?? "多人合作",
        })}
      />
    </section>
  );
}

function compareByUpdatedAtDesc(left: GameCard, right: GameCard) {
  return toTimestamp(right.userState.updatedAt) - toTimestamp(left.userState.updatedAt);
}

function toTimestamp(value?: string | null) {
  if (!value) {
    return Number.NEGATIVE_INFINITY;
  }

  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? Number.NEGATIVE_INFINITY : parsed;
}

function formatUpdatedAt(value?: string | null) {
  if (!value) {
    return "最近浏览时间未知";
  }

  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return "最近浏览时间未知";
  }

  return `最近浏览 · ${parsed.toLocaleString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    month: "2-digit",
    day: "2-digit",
  })}`;
}
