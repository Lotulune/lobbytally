import type { AiAssessment, GameCard } from "../../types";

export function AiAssistantPage({
  games,
  assessment,
  selectedGame,
  isBusy,
  onAssess,
}: {
  games: GameCard[];
  assessment: AiAssessment | null;
  selectedGame: GameCard | null;
  isBusy: boolean;
  onAssess: (game: GameCard) => void;
}) {
  return (
    <section className="ai-page">
      <div className="assistant-head">
        <h2>
          ✨ AI 智能推荐助手 <em>Beta</em>
        </h2>
        <button type="button">历史记录</button>
      </div>
      <div className="chat-bubble user">
        我想找一款可以和朋友轻松玩、画风可爱、支持本地合作的游戏
      </div>
      <div className="chat-bubble bot">好的！根据你的需求，我为你找到了以下游戏推荐：</div>

      <div className="recommend-list">
        {games.map((game) => (
          <article className="recommend-row" key={game.appid}>
            <img src={game.capsuleUrl} alt="" />
            <div>
              <h3>{game.name}</h3>
              <p>
                {game.tags.join(" · ")} · {formatPct(game.positiveReviewPct)} 好评
              </p>
              <span>{game.aiSummary}</span>
            </div>
            <strong>
              {Math.round(game.recommendationScore)}
              <small>推荐值</small>
            </strong>
            <button type="button" disabled={isBusy} onClick={() => onAssess(game)}>
              {isBusy && selectedGame?.appid === game.appid ? "评估中" : "评估"}
            </button>
          </article>
        ))}
      </div>

      <div className="chat-input">
        <span>
          {assessment?.summary ??
            "描述你想要的游戏，例如：和朋友一起生存、像素风格、5 人以上..."}
        </span>
        <button type="button">➤</button>
      </div>
    </section>
  );
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}
