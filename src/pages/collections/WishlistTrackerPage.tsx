import type { GameCard, UserGameStatePatch } from "../../types";
import { CollectionGrid } from "./CollectionGrid";

export function WishlistTrackerPage({
  games,
  onOpen,
  onToggle,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
  onToggle: (game: GameCard, patch: UserGameStatePatch, message: string) => void;
}) {
  return (
    <section className="favorites-page">
      <div className="collection-page-head">
        <div>
          <h2>愿望单追踪</h2>
          <p>这里直接读取本地用户状态里的 wishlist 标记，方便你回看准备和朋友一起开的坑。</p>
        </div>
      </div>
      <CollectionGrid
        actionLabel={(game) => `移出愿望单《${game.name}》`}
        countLabel={`愿望单 · ${games.length} 款`}
        emptyBody="还没有加入愿望单的游戏。去详情页点一下“愿望单”，这里会立刻同步你的本地状态。"
        emptyTitle="愿望单还是空的"
        games={games}
        onAction={(game) =>
          onToggle(game, { wishlist: false }, `已从愿望单移除《${game.name}》。`)
        }
        onOpen={onOpen}
      />
    </section>
  );
}
