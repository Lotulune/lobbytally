import type { GameCard } from "../../types";

export function UpcomingPage({
  games,
  onOpen,
  onToggleFollow,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
  onToggleFollow: (game: GameCard) => void;
}) {
  return (
    <section className="upcoming-page">
      <div className="upcoming-hero">
        <div>
          <h2>即将上线</h2>
          <p>
            优先关注未来发售、即将开放 Demo、或待公布状态的多人游戏，方便提前加愿望单或关注上线提醒。
          </p>
        </div>
        <div className="upcoming-summary">
          <strong>{games.length}</strong>
          <span>款候选</span>
        </div>
      </div>

      {games.length === 0 ? (
        <div className="upcoming-empty">
          <h3>还没有符合条件的即将上线游戏</h3>
          <p>可以先放宽语言、时间窗口或好评度筛选条件，再回来看看有哪些多人新作值得蹲。</p>
        </div>
      ) : (
        <div className="upcoming-grid">
          {games.map((game) => (
            <article className="upcoming-card" key={game.appid}>
              <button
                className="upcoming-card-media"
                type="button"
                onClick={() => onOpen(game)}
              >
                <img src={game.capsuleUrl} alt={game.name} />
              </button>

              <div className="upcoming-card-body">
                <div className="upcoming-card-eyebrow">
                  <span>{releaseStateLabel(game.releaseState)}</span>
                  {typeof game.discountPercent === "number" && game.discountPercent > 0 ? (
                    <b>{game.discountPercent}% OFF</b>
                  ) : null}
                </div>

                <h3>{game.name}</h3>
                <p>{formatReleaseLine(game)}</p>
                <span>{formatLanguageLine(game.supportedLanguages)}</span>

                <div className="upcoming-card-actions">
                  <button
                    className="muted-button"
                    type="button"
                    onClick={() => onOpen(game)}
                  >
                    查看详情
                  </button>
                  <button
                    aria-label={`${game.userState.followed ? "取消关注" : "关注上线"}《${game.name}》`}
                    className="gold-button"
                    type="button"
                    onClick={() => onToggleFollow(game)}
                  >
                    {game.userState.followed ? "已关注" : "关注上线"}
                  </button>
                </div>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function releaseStateLabel(state: GameCard["releaseState"]) {
  switch (state) {
    case "upcoming":
      return "即将发售";
    case "tba":
      return "待公布";
    case "released":
      return "已发售";
  }
}

function formatReleaseLine(game: GameCard) {
  const countdown = releaseCountdown(game.releaseDate);
  const releaseText = game.releaseDateText?.trim() || "发售日待公布";
  const priceText = game.priceText?.trim() || "价格待定";
  return `${releaseText} · ${countdown} · ${priceText}`;
}

function formatLanguageLine(languages: string[]) {
  if (languages.length === 0) {
    return "语言信息待补充";
  }

  return languages
    .slice(0, 3)
    .map((language) => {
      switch (language.toLowerCase()) {
        case "schinese":
          return "简体中文";
        case "english":
          return "英语";
        default:
          return language;
      }
    })
    .join(" / ");
}

function releaseCountdown(releaseDate?: string | null) {
  if (!releaseDate) return "日期待公布";

  const today = new Date();
  const target = new Date(`${releaseDate}T00:00:00Z`);
  if (Number.isNaN(target.getTime())) return "日期待公布";

  const todayUtc = Date.UTC(
    today.getUTCFullYear(),
    today.getUTCMonth(),
    today.getUTCDate(),
  );
  const days = Math.ceil((target.getTime() - todayUtc) / 86_400_000);

  if (days > 1) return `还有 ${days} 天`;
  if (days === 1) return "明天可玩";
  if (days === 0) return "今天发售";
  return "已可发售";
}
