import type { ReactNode } from "react";
import type { GameCard } from "../../types";

export function CollectionGrid({
  actionLabel,
  countLabel,
  emptyBody,
  emptyTitle,
  games,
  onAction,
  onOpen,
  renderBadge,
  renderMeta,
}: {
  actionLabel?: (game: GameCard) => string;
  countLabel: string;
  emptyBody: string;
  emptyTitle: string;
  games: GameCard[];
  onAction?: (game: GameCard) => void;
  onOpen: (game: GameCard) => void;
  renderBadge?: (game: GameCard) => ReactNode;
  renderMeta?: (game: GameCard) => { primary: string; secondary?: string };
}) {
  return (
    <>
      <div className="favorite-toolbar">
        <button type="button">{countLabel}</button>
        <button type="button">最近添加⌄</button>
        <button type="button">▦</button>
        <button type="button">☷</button>
      </div>
      {games.length === 0 ? (
        <div className="empty-collection">
          <LogoMark />
          <h3>{emptyTitle}</h3>
          <p>{emptyBody}</p>
        </div>
      ) : (
        <div className="favorite-grid">
          {games.map((game) => {
            const meta = renderMeta?.(game) ?? defaultMeta(game);

            return (
              <article className="favorite-card" key={game.appid} onClick={() => onOpen(game)}>
                <img src={game.capsuleUrl} alt="" />
                <div className="favorite-card-body">
                  {renderBadge ? <div className="favorite-card-badge">{renderBadge(game)}</div> : null}
                  <h3>{game.name}</h3>
                  <p>{meta.primary}</p>
                  {meta.secondary ? <span>{meta.secondary}</span> : null}
                </div>
                {onAction ? (
                  <button
                    aria-label={actionLabel?.(game) ?? `移出《${game.name}》`}
                    type="button"
                    onClick={(event) => {
                      event.stopPropagation();
                      onAction(game);
                    }}
                  >
                    ×
                  </button>
                ) : null}
              </article>
            );
          })}
        </div>
      )}
    </>
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

function defaultMeta(game: GameCard) {
  return {
    primary: `${formatPct(game.positiveReviewPct)} 好评`,
    secondary: game.multiplayerModes[0] ?? "多人合作",
  };
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}
