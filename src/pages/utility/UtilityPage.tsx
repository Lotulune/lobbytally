import type { DashboardPayload, GameCard } from "../../types";
import type { ViewId } from "../types";

export function UtilityPage({
  view,
  collections,
  games,
  onOpen,
}: {
  view: ViewId;
  collections: DashboardPayload["collections"];
  games: GameCard[];
  onOpen: (game: GameCard) => void;
}) {
  if (view === "wishlist") {
    return (
      <MiniCollectionPage
        title="愿望单追踪"
        body="这些游戏已加入愿望单，后续可接 Steam 愿望单导入和发售/Demo 提醒。"
        games={collections.wishlist}
        emptyGames={games.slice(0, 4)}
        onOpen={onOpen}
      />
    );
  }

  if (view === "history") {
    return (
      <MiniCollectionPage
        title="游玩记录"
        body="这里记录你打开过详情页的游戏，用来反向推断最近兴趣。"
        games={collections.history}
        emptyGames={games.slice(0, 4)}
        onOpen={onOpen}
      />
    );
  }

  const copy: Partial<Record<ViewId, [string, string]>> = {
    upcoming: ["即将上线", "这里会追踪已上架但未发售、即将开放 Demo 的多人游戏。"],
    wishlist: ["愿望单追踪", "后续可以导入 Steam 愿望单，提示支持多人联机的新变化。"],
    history: ["游玩记录", "这里适合接 Steam 最近游玩记录，反向推荐朋友也可能喜欢的游戏。"],
    about: ["关于 Co-Play", "一个专门为多人联机游戏做发现和推荐的小应用。"],
  };

  const [title, body] = copy[view] ?? ["页面准备中", "这个模块已经在导航中占位。"];
  return (
    <section className="placeholder-page">
      <LogoMark />
      <h2>{title}</h2>
      <p>{body}</p>
    </section>
  );
}

function MiniCollectionPage({
  title,
  body,
  games,
  emptyGames,
  onOpen,
}: {
  title: string;
  body: string;
  games: GameCard[];
  emptyGames: GameCard[];
  onOpen: (game: GameCard) => void;
}) {
  const visibleGames = games.length > 0 ? games : emptyGames;
  return (
    <section className="placeholder-page">
      <h2>{title}</h2>
      <p>
        {games.length > 0
          ? body
          : `${body} 现在先展示推荐占位；添加后会替换为你的真实列表。`}
      </p>
      <div className="favorite-grid mini">
        {visibleGames.map((game) => (
          <article className="favorite-card" key={game.appid} onClick={() => onOpen(game)}>
            <img src={game.capsuleUrl} alt="" />
            <h3>{game.name}</h3>
            <p>{formatPct(game.positiveReviewPct)} 好评</p>
            <span>{game.multiplayerModes[0] ?? "多人合作"}</span>
          </article>
        ))}
      </div>
    </section>
  );
}

function LogoMark() {
  return (
    <span className="logo-mark" aria-hidden="true">
      <i />
      <i />
      <b />
    </span>
  );
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}
