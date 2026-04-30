import { describe, expect, it } from "vitest";
import {
  bucketGame,
  scoreGame,
  type GameRecommendationFacts,
} from "./recommendation";

const baseFacts: GameRecommendationFacts = {
  appid: 1,
  name: "Moonlit Co-op",
  releaseDate: "2026-04-20",
  positiveReviewPct: 92,
  totalReviews: 820,
  currentPlayers: 1200,
  multiplayerModes: ["Online Co-op", "Co-op"],
  demoStatus: "released_with_demo",
  aiScore: 88,
};

describe("recommendation scoring", () => {
  it("rewards strong reviews, active players, multiplayer fit, demo availability, and AI score", () => {
    const score = scoreGame(baseFacts, new Date("2026-04-26T00:00:00Z"));

    expect(score).toBeGreaterThanOrEqual(88);
    expect(score).toBeLessThanOrEqual(94);
  });

  it("keeps low-player hidden gems visible when reviews are excellent and multiplayer fit is clear", () => {
    const score = scoreGame(
      {
        ...baseFacts,
        positiveReviewPct: 97,
        totalReviews: 180,
        currentPlayers: 18,
        aiScore: 91,
      },
      new Date("2026-04-26T00:00:00Z"),
    );

    expect(score).toBeGreaterThanOrEqual(80);
  });

  it("places games from the last 30 days into the new release bucket", () => {
    expect(bucketGame(baseFacts, new Date("2026-04-26T00:00:00Z"))).toBe(
      "new",
    );
  });

  it("places older multiplayer games into the classics bucket", () => {
    expect(
      bucketGame(
        { ...baseFacts, releaseDate: "2020-05-13" },
        new Date("2026-04-26T00:00:00Z"),
      ),
    ).toBe("classic");
  });
});
