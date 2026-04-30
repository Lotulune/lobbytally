export type DemoStatus =
  | "demo_only"
  | "released_with_demo"
  | "released"
  | "unknown";

export type ReleaseBucket = "new" | "classic";

export interface GameRecommendationFacts {
  appid: number;
  name: string;
  releaseDate?: string | null;
  positiveReviewPct?: number | null;
  totalReviews?: number | null;
  currentPlayers?: number | null;
  multiplayerModes: string[];
  demoStatus: DemoStatus;
  aiScore?: number | null;
}

export function scoreGame(
  facts: GameRecommendationFacts,
  today: Date = new Date(),
): number {
  const reviewQuality = clamp(facts.positiveReviewPct ?? 0, 0, 100) / 100 * 36;
  const reviewConfidence = logWeight(facts.totalReviews ?? 0, 10_000) * 8;
  const playerActivity = logWeight(facts.currentPlayers ?? 0, 10_000) * 14;
  const multiplayerFit = multiplayerFitScore(facts.multiplayerModes);
  const demoBonus = demoBonusScore(facts.demoStatus);
  const freshness = freshnessScore(facts.releaseDate, today);
  const aiScore = clamp(facts.aiScore ?? 72, 0, 100) / 100 * 20;

  return clamp(
    roundOne(
      reviewQuality +
        reviewConfidence +
        playerActivity +
        multiplayerFit +
        demoBonus +
        freshness +
        aiScore,
    ),
    0,
    100,
  );
}

export function bucketGame(
  facts: Pick<GameRecommendationFacts, "releaseDate">,
  today: Date = new Date(),
): ReleaseBucket {
  const days = daysSinceRelease(facts.releaseDate, today);
  return days !== null && days >= 0 && days <= 30 ? "new" : "classic";
}

function multiplayerFitScore(modes: string[]): number {
  if (modes.length === 0) return 0;

  const normalized = modes.join(" ").toLowerCase();
  let score = 8;
  if (normalized.includes("co-op") || normalized.includes("cooperative")) {
    score += 4;
  }
  if (
    normalized.includes("online") ||
    normalized.includes("lan") ||
    normalized.includes("multi-player")
  ) {
    score += 2;
  }
  return clamp(score, 0, 14);
}

function demoBonusScore(status: DemoStatus): number {
  switch (status) {
    case "demo_only":
    case "released_with_demo":
      return 4;
    case "released":
      return 1.5;
    case "unknown":
      return 0;
  }
}

function freshnessScore(releaseDate: string | null | undefined, today: Date) {
  const days = daysSinceRelease(releaseDate, today);
  if (days === null) return 0;
  if (days >= 0 && days <= 7) return 5;
  if (days <= 30) return 4;
  if (days <= 180) return 1.5;
  return 0;
}

function daysSinceRelease(
  releaseDate: string | null | undefined,
  today: Date,
): number | null {
  if (!releaseDate) return null;
  const release = new Date(`${releaseDate}T00:00:00Z`);
  if (Number.isNaN(release.getTime())) return null;
  const todayUtc = Date.UTC(
    today.getUTCFullYear(),
    today.getUTCMonth(),
    today.getUTCDate(),
  );
  return Math.floor((todayUtc - release.getTime()) / 86_400_000);
}

function logWeight(value: number, maxReference: number): number {
  if (value <= 0) return 0;
  return clamp(Math.log10(value + 1) / Math.log10(maxReference + 1), 0, 1);
}

function roundOne(value: number): number {
  return Math.round(value * 10) / 10;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
