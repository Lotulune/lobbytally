import { useState } from "react";
import type { DashboardPayload, GameCard, UserGameStatePatch } from "../../types";
import { CollectionGrid } from "./CollectionGrid";

type CollectionTab = "favorites" | "wishlist" | "followed" | "history";

const collectionTabCopy: Record<
  CollectionTab,
  { emptyTitle: string; emptyCopy: string; label: string }
> = {
  favorites: {
    emptyTitle: "游戏收藏 还是空的",
    emptyCopy: "去详情页点击“收藏”，这里会立刻同步本地 SQLite 状态。",
    label: "游戏收藏",
  },
  wishlist: {
    emptyTitle: "愿望单 还是空的",
    emptyCopy: "去详情页点击“加入愿望单”，这里会立刻同步本地 SQLite 状态。",
    label: "愿望单",
  },
  followed: {
    emptyTitle: "关注的游戏 还是空的",
    emptyCopy: "去详情页点击“关注”，这里会立刻同步本地 SQLite 状态。",
    label: "关注的游戏",
  },
  history: {
    emptyTitle: "浏览记录 还是空的",
    emptyCopy: "去详情页点击“收藏 / 加入愿望单 / 关注”，这里会立刻同步本地 SQLite 状态。",
    label: "浏览记录",
  },
};

export function CollectionsHubPage({
  collections,
  onOpen,
  onToggle,
}: {
  collections: DashboardPayload["collections"];
  onOpen: (game: GameCard) => void;
  onToggle: (game: GameCard, patch: UserGameStatePatch, message: string) => void;
}) {
  const [tab, setTab] = useState<CollectionTab>("favorites");
  const games = collections[tab];
  const copy = collectionTabCopy[tab];
  const visibleGames =
    tab === "history" ? [...games].sort(compareByUpdatedAtDesc) : games;

  return (
    <section className="favorites-page collection-page">
      <div className="favorites-head">
        <h2>我的收藏夹</h2>
        <div>
          <button
            className={tab === "favorites" ? "active" : ""}
            type="button"
            onClick={() => setTab("favorites")}
          >
            游戏收藏
          </button>
          <button
            className={tab === "wishlist" ? "active" : ""}
            type="button"
            onClick={() => setTab("wishlist")}
          >
            愿望单
          </button>
          <button
            className={tab === "followed" ? "active" : ""}
            type="button"
            onClick={() => setTab("followed")}
          >
            关注的游戏
          </button>
          <button
            className={tab === "history" ? "active" : ""}
            type="button"
            onClick={() => setTab("history")}
          >
            浏览记录
          </button>
        </div>
      </div>
      <CollectionGrid
        actionLabel={(game) => `移出${copy.label}《${game.name}》`}
        countLabel={`${copy.label} · ${visibleGames.length} 款`}
        emptyBody={copy.emptyCopy}
        emptyTitle={copy.emptyTitle}
        games={visibleGames}
        onAction={(game) =>
          onToggle(
            game,
            removalPatchByTab(tab),
            `已从${copy.label}移除《${game.name}》。`,
          )
        }
        onOpen={onOpen}
        renderBadge={
          tab === "history"
            ? (game) => (
                <span className="favorite-history-stamp">
                  {formatUpdatedAt(game.userState.updatedAt)}
                </span>
              )
            : undefined
        }
        renderMeta={
          tab === "history"
            ? (game) => ({
                primary: formatUpdatedAt(game.userState.updatedAt),
                secondary: game.multiplayerModes[0] ?? "多人合作",
              })
            : (game) => ({
                primary: `${formatPct(game.positiveReviewPct)} 好评`,
                secondary: game.multiplayerModes[0] ?? "多人合作",
              })
        }
      />
    </section>
  );
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}

function removalPatchByTab(tab: CollectionTab): UserGameStatePatch {
  switch (tab) {
    case "wishlist":
      return { wishlist: false };
    case "followed":
      return { followed: false };
    case "history":
      return { viewed: false };
    case "favorites":
      return { favorite: false };
  }
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
