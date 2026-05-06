import type { DashboardPayload, GameAnalysisReport, GameCard } from "../../types";

export interface GameAnalysisSnapshot {
  appid: number;
  aiScore: number;
  aiSummary: string;
}

export function snapshotFromReport(
  report: Pick<
    GameAnalysisReport,
    "appid" | "overallScore" | "overview" | "recommendationScore"
  >,
): GameAnalysisSnapshot {
  return {
    appid: report.appid,
    aiScore:
      typeof report.recommendationScore === "number"
        ? report.recommendationScore
        : report.overallScore,
    aiSummary: report.overview,
  };
}

export function applyGameAnalysisSnapshotToGame<T extends Pick<GameCard, "appid" | "aiScore" | "aiSummary">>(
  game: T,
  snapshot: GameAnalysisSnapshot,
) {
  if (game.appid !== snapshot.appid) {
    return game;
  }

  return {
    ...game,
    aiScore: snapshot.aiScore,
    aiSummary: snapshot.aiSummary,
  };
}

export function applyGameAnalysisSnapshotToDashboard(
  dashboard: DashboardPayload,
  snapshot: GameAnalysisSnapshot,
): DashboardPayload {
  return {
    ...dashboard,
    newGames: dashboard.newGames.map((game) =>
      applyGameAnalysisSnapshotToGame(game, snapshot),
    ),
    classics: dashboard.classics.map((game) =>
      applyGameAnalysisSnapshotToGame(game, snapshot),
    ),
    hiddenGames: dashboard.hiddenGames.map((game) =>
      applyGameAnalysisSnapshotToGame(game, snapshot),
    ),
    upcoming: dashboard.upcoming.map((game) =>
      applyGameAnalysisSnapshotToGame(game, snapshot),
    ),
    recentDiscoveries: dashboard.recentDiscoveries.map((game) =>
      applyGameAnalysisSnapshotToGame(game, snapshot),
    ),
    collections: {
      favorites: dashboard.collections.favorites.map((game) =>
        applyGameAnalysisSnapshotToGame(game, snapshot),
      ),
      wishlist: dashboard.collections.wishlist.map((game) =>
        applyGameAnalysisSnapshotToGame(game, snapshot),
      ),
      followed: dashboard.collections.followed.map((game) =>
        applyGameAnalysisSnapshotToGame(game, snapshot),
      ),
      history: dashboard.collections.history.map((game) =>
        applyGameAnalysisSnapshotToGame(game, snapshot),
      ),
    },
  };
}
