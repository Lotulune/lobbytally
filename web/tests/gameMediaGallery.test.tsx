import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GameMediaBlock } from "../src/api/types";
import {
  buildGalleryItems,
  GameMediaGallery,
} from "../src/components/GameMediaGallery";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const hlsTestState = vi.hoisted(() => ({
  instances: 0,
  destroyed: 0,
  sources: [] as string[],
  importBarrier: null as Promise<void> | null,
}));

vi.mock("hls.js", async () => {
  if (hlsTestState.importBarrier) await hlsTestState.importBarrier;

  class MockHls {
    static Events = { ERROR: "error" };

    static isSupported() {
      return true;
    }

    constructor() {
      hlsTestState.instances += 1;
    }

    loadSource(source: string) {
      hlsTestState.sources.push(source);
    }

    attachMedia() {}

    on() {}

    destroy() {
      hlsTestState.destroyed += 1;
    }
  }

  return { default: MockHls };
});

const fullMedia: GameMediaBlock = {
  updated_at_ms: 1_700_000_000_000,
  screenshots: [
    {
      id: "0",
      thumbnail_url: "https://shared.akamai.steamstatic.com/t0.jpg",
      full_url: "https://shared.akamai.steamstatic.com/f0.jpg",
    },
    {
      id: "1",
      thumbnail_url: "https://shared.akamai.steamstatic.com/t1.jpg",
      full_url: "https://shared.akamai.steamstatic.com/f1.jpg",
    },
  ],
  videos: [
    {
      id: "v1",
      title: "Highlight Trailer",
      poster_url: "https://shared.akamai.steamstatic.com/p1.jpg",
      highlight: true,
      mp4_url: "https://video.akamai.steamstatic.com/a.mp4",
      hls_h264_url: null,
      dash_h264_url: null,
    },
    {
      id: "v2",
      title: "Other Trailer",
      poster_url: "https://shared.akamai.steamstatic.com/p2.jpg",
      highlight: false,
      mp4_url: null,
      hls_h264_url: "https://video.akamai.steamstatic.com/b.m3u8",
      dash_h264_url: null,
    },
  ],
};

function mountGallery(props: {
  coverUrl?: string | null;
  media?: GameMediaBlock | null;
  steamUrl?: string;
}) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  act(() => {
    root.render(
      <GameMediaGallery
        appId={892970}
        name="Valheim"
        coverUrl={props.coverUrl ?? "https://shared.akamai.steamstatic.com/header.jpg"}
        media={props.media}
        steamUrl={props.steamUrl ?? "https://store.steampowered.com/app/892970/"}
      />,
    );
  });
  return {
    host,
    root,
    unmount() {
      act(() => root.unmount());
      host.remove();
    },
  };
}

beforeEach(() => {
  hlsTestState.instances = 0;
  hlsTestState.destroyed = 0;
  hlsTestState.sources = [];
  hlsTestState.importBarrier = null;
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("buildGalleryItems", () => {
  it("orders cover, highlight videos, screenshots, then other videos", () => {
    const items = buildGalleryItems({
      appId: 892970,
      name: "Valheim",
      coverUrl: "https://shared.akamai.steamstatic.com/header.jpg",
      media: fullMedia,
    });
    expect(items.map((i) => i.id)).toEqual([
      "cover",
      "video-v1",
      "shot-0",
      "shot-1",
      "video-v2",
    ]);
  });

  it("treats missing media as empty and still shows cover", () => {
    const items = buildGalleryItems({
      appId: 892970,
      name: "Valheim",
      coverUrl: "https://shared.akamai.steamstatic.com/header.jpg",
      media: undefined,
    });
    expect(items).toHaveLength(1);
    expect(items[0]?.kind).toBe("cover");
  });

  it("dedupes cover URL against identical screenshot full URL", () => {
    const items = buildGalleryItems({
      appId: 1,
      name: "Game",
      coverUrl: "https://shared.akamai.steamstatic.com/same.jpg",
      media: {
        updated_at_ms: 1,
        screenshots: [
          {
            id: "9",
            thumbnail_url: "https://shared.akamai.steamstatic.com/same-t.jpg",
            full_url: "https://shared.akamai.steamstatic.com/same.jpg",
          },
        ],
        videos: [],
      },
    });
    expect(items.filter((i) => i.kind === "screenshot")).toHaveLength(0);
    expect(items[0]?.kind).toBe("cover");
  });
});

describe("GameMediaGallery", () => {
  it("renders only cover when media is missing (old server)", () => {
    const { host, unmount } = mountGallery({ media: undefined });
    expect(host.querySelector('[data-testid="game-media-gallery"]')).toBeTruthy();
    expect(host.querySelectorAll(".gallery-thumb")).toHaveLength(0);
    expect(host.querySelector(".gallery-stage-image img")?.getAttribute("src")).toContain(
      "header.jpg",
    );
    unmount();
  });

  it("renders empty media arrays as cover-only experience", () => {
    const { host, unmount } = mountGallery({
      media: { updated_at_ms: null, screenshots: [], videos: [] },
    });
    expect(host.querySelectorAll(".gallery-thumb")).toHaveLength(0);
    unmount();
  });

  it("falls back through Steam cover CDN candidates before removing the cover", () => {
    const { host, unmount } = mountGallery({
      coverUrl: "https://shared.akamai.steamstatic.com/broken.jpg",
      media: { updated_at_ms: null, screenshots: [], videos: [] },
    });
    const image = host.querySelector<HTMLImageElement>(".gallery-stage-image img")!;

    act(() => {
      image.dispatchEvent(new Event("error"));
    });

    expect(image.getAttribute("src")).toBe(
      "https://cdn.akamai.steamstatic.com/steam/apps/892970/header.jpg",
    );
    expect(host.querySelector(".gallery-fallback")).toBeNull();
    unmount();
  });

  it("supports click and keyboard thumbnail switching with accessible current state", () => {
    const { host, unmount } = mountGallery({ media: fullMedia });
    const thumbs = host.querySelectorAll<HTMLButtonElement>(".gallery-thumb");
    expect(thumbs.length).toBeGreaterThan(1);
    expect(thumbs[0]?.getAttribute("aria-selected")).toBe("true");
    expect(thumbs[0]?.getAttribute("aria-current")).toBe("true");

    act(() => {
      thumbs[2]?.click();
    });
    expect(thumbs[2]?.getAttribute("aria-selected")).toBe("true");

    act(() => {
      host.querySelector(".gallery-rail")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }),
      );
    });
    expect(thumbs[3]?.getAttribute("aria-selected")).toBe("true");

    act(() => {
      host.querySelector(".gallery-rail")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true }),
      );
    });
    expect(thumbs[0]?.getAttribute("aria-selected")).toBe("true");

    act(() => {
      host.querySelector(".gallery-rail")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }),
      );
    });
    expect(thumbs[thumbs.length - 1]?.getAttribute("aria-selected")).toBe("true");
    unmount();
  });

  it("skips failed images without breaking the gallery", () => {
    const { host, unmount } = mountGallery({ media: fullMedia });
    const mainImg = host.querySelector<HTMLImageElement>(".gallery-stage-image img")!;

    act(() => {
      mainImg.dispatchEvent(new Event("error"));
    });
    expect(host.querySelector(".gallery-stage-image")).toBeTruthy();

    // Exhaust the four remaining Steam CDN candidates before dropping the cover.
    for (let attempt = 0; attempt < 4; attempt += 1) {
      act(() => {
        mainImg.dispatchEvent(new Event("error"));
      });
    }
    expect(host.querySelector(".gallery-stage-video")).toBeTruthy();
    unmount();
  });

  it("does not load video sources until the user clicks play", () => {
    const { host, unmount } = mountGallery({
      media: {
        updated_at_ms: 1,
        screenshots: [],
        videos: [
          {
            id: "only",
            title: "Trailer",
            poster_url: "https://shared.akamai.steamstatic.com/p.jpg",
            highlight: true,
            mp4_url: "https://video.akamai.steamstatic.com/only.mp4",
            hls_h264_url: null,
            dash_h264_url: null,
          },
        ],
      },
      coverUrl: null,
    });
    // Without a cover, highlight video is first; no src until play.
    // Force select video if cover CDN candidates exist from appId.
    const videoThumb = Array.from(host.querySelectorAll<HTMLButtonElement>(".gallery-thumb")).find(
      (btn) => btn.classList.contains("is-video"),
    );
    if (videoThumb) {
      act(() => videoThumb.click());
    }
    const video = host.querySelector<HTMLVideoElement>(".gallery-video")!;
    expect(video.getAttribute("src")).toBeNull();
    expect(video.autoplay).toBe(false);
    expect(host.querySelector(".gallery-play-btn")).toBeTruthy();
    unmount();
  });

  it("shows Steam fallback when playback cannot start", async () => {
    const playSpy = vi
      .spyOn(HTMLMediaElement.prototype, "play")
      .mockRejectedValue(new Error("not allowed"));
    const { host, unmount } = mountGallery({
      media: {
        updated_at_ms: 1,
        screenshots: [],
        videos: [
          {
            id: "only",
            title: "Trailer",
            poster_url: "https://shared.akamai.steamstatic.com/p.jpg",
            highlight: true,
            mp4_url: "https://video.akamai.steamstatic.com/only.mp4",
            hls_h264_url: null,
            dash_h264_url: null,
          },
        ],
      },
    });
    const videoThumb = Array.from(host.querySelectorAll<HTMLButtonElement>(".gallery-thumb")).find(
      (btn) => btn.classList.contains("is-video"),
    );
    if (videoThumb) {
      act(() => videoThumb.click());
    }
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".gallery-play-btn")?.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(host.textContent).toContain("当前环境无法播放预告片");
    expect(host.querySelector('a[href*="store.steampowered.com"]')).toBeTruthy();
    playSpy.mockRestore();
    unmount();
  });

  it("recreates the video stage when switching between video items", async () => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
    const { host, unmount } = mountGallery({
      media: {
        updated_at_ms: 1,
        screenshots: [],
        videos: [
          {
            id: "first",
            title: "First Trailer",
            poster_url: "https://shared.akamai.steamstatic.com/first.jpg",
            highlight: true,
            mp4_url: "https://video.akamai.steamstatic.com/first.mp4",
            hls_h264_url: null,
            dash_h264_url: null,
          },
          {
            id: "second",
            title: "Second Trailer",
            poster_url: "https://shared.akamai.steamstatic.com/second.jpg",
            highlight: true,
            mp4_url: "https://video.akamai.steamstatic.com/second.mp4",
            hls_h264_url: null,
            dash_h264_url: null,
          },
        ],
      },
    });
    const firstThumb = host.querySelector<HTMLButtonElement>(
      '.gallery-thumb[aria-label="预告片：First Trailer"]',
    )!;
    const secondThumb = host.querySelector<HTMLButtonElement>(
      '.gallery-thumb[aria-label="预告片：Second Trailer"]',
    )!;

    act(() => firstThumb.click());
    const firstVideo = host.querySelector<HTMLVideoElement>(".gallery-video")!;
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".gallery-play-btn")!.click();
      await Promise.resolve();
    });
    expect(firstVideo.getAttribute("src")).toBe(
      "https://video.akamai.steamstatic.com/first.mp4",
    );

    act(() => secondThumb.click());
    const secondVideo = host.querySelector<HTMLVideoElement>(".gallery-video")!;
    expect(secondVideo).not.toBe(firstVideo);
    expect(secondVideo.getAttribute("src")).toBeNull();
    expect(host.querySelector(".gallery-play-btn")).toBeTruthy();
    unmount();
  });

  it("does not attach a stale HLS source after switching video items", async () => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
    vi.spyOn(HTMLMediaElement.prototype, "canPlayType").mockReturnValue("");
    let releaseImport = () => {};
    hlsTestState.importBarrier = new Promise<void>((resolve) => {
      releaseImport = resolve;
    });
    const { host, unmount } = mountGallery({
      media: {
        updated_at_ms: 1,
        screenshots: [],
        videos: [
          {
            id: "first-hls",
            title: "First HLS",
            poster_url: "https://shared.akamai.steamstatic.com/first-hls.jpg",
            highlight: true,
            mp4_url: null,
            hls_h264_url: "https://video.akamai.steamstatic.com/first.m3u8",
            dash_h264_url: null,
          },
          {
            id: "second-hls",
            title: "Second HLS",
            poster_url: "https://shared.akamai.steamstatic.com/second-hls.jpg",
            highlight: true,
            mp4_url: null,
            hls_h264_url: "https://video.akamai.steamstatic.com/second.m3u8",
            dash_h264_url: null,
          },
        ],
      },
    });
    const firstThumb = host.querySelector<HTMLButtonElement>(
      '.gallery-thumb[aria-label="预告片：First HLS"]',
    )!;
    const secondThumb = host.querySelector<HTMLButtonElement>(
      '.gallery-thumb[aria-label="预告片：Second HLS"]',
    )!;

    act(() => firstThumb.click());
    act(() => {
      host.querySelector<HTMLButtonElement>(".gallery-play-btn")!.click();
    });
    act(() => secondThumb.click());
    releaseImport();
    await act(async () => {
      await vi.dynamicImportSettled();
      await Promise.resolve();
    });

    expect(hlsTestState.instances).toBe(0);
    expect(hlsTestState.sources).toEqual([]);
    expect(host.querySelector(".gallery-play-btn")).toBeTruthy();

    await act(async () => {
      host.querySelector<HTMLButtonElement>(".gallery-play-btn")!.click();
      await vi.dynamicImportSettled();
      await Promise.resolve();
    });
    expect(hlsTestState.instances).toBe(1);
    expect(hlsTestState.sources).toEqual([
      "https://video.akamai.steamstatic.com/second.m3u8",
    ]);
    unmount();
  });

  it("deduplicates concurrent HLS startup and destroys the active player on unmount", async () => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
    vi.spyOn(HTMLMediaElement.prototype, "canPlayType").mockReturnValue("");
    const { host, unmount } = mountGallery({
      media: {
        updated_at_ms: 1,
        screenshots: [],
        videos: [
          {
            id: "hls-only",
            title: "HLS Trailer",
            poster_url: "https://shared.akamai.steamstatic.com/p.jpg",
            highlight: true,
            mp4_url: null,
            hls_h264_url: "https://video.akamai.steamstatic.com/only.m3u8",
            dash_h264_url: null,
          },
        ],
      },
    });
    const videoThumb = Array.from(host.querySelectorAll<HTMLButtonElement>(".gallery-thumb")).find(
      (button) => button.classList.contains("is-video"),
    )!;
    act(() => videoThumb.click());

    const play = host.querySelector<HTMLButtonElement>(".gallery-play-btn")!;
    act(() => {
      play.click();
    });
    expect(play.disabled).toBe(true);
    // A disabled loading control must not start a second dynamic HLS initialization.
    play.click();
    await act(async () => {
      await vi.dynamicImportSettled();
      await Promise.resolve();
    });

    expect(hlsTestState.instances).toBe(1);
    unmount();
    expect(hlsTestState.destroyed).toBe(1);
  });

  it("opens a full-viewport zoom lightbox and Escape closes without going back", () => {
    const { host, unmount } = mountGallery({ media: fullMedia });
    const underlying = vi.fn();
    window.addEventListener("keydown", underlying);

    act(() => {
      host.querySelector<HTMLButtonElement>(".gallery-stage-image")?.click();
    });
    const lightbox = document.querySelector<HTMLElement>(".gallery-lightbox");
    expect(lightbox).toBeTruthy();
    expect(lightbox?.getAttribute("role")).toBe("dialog");
    expect(lightbox?.getAttribute("aria-modal")).toBe("true");
    expect(document.querySelector(".gallery-lightbox-img")).toBeTruthy();
    // Must not look like the form Modal card.
    expect(document.querySelector(".modal-backdrop")).toBeNull();
    expect(document.querySelector(".modal-head")).toBeNull();

    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(document.querySelector(".gallery-lightbox")).toBeNull();
    // Capture-phase handler should prevent the detail page from treating Escape as back.
    expect(underlying).not.toHaveBeenCalled();

    window.removeEventListener("keydown", underlying);
    unmount();
  });

  it("supports wheel zoom scale changes in the lightbox", () => {
    const { host, unmount } = mountGallery({ media: fullMedia });
    act(() => {
      host.querySelector<HTMLButtonElement>(".gallery-stage-image")?.click();
    });

    const stage = document.querySelector<HTMLDivElement>(".gallery-lightbox-stage")!;
    const img = document.querySelector<HTMLImageElement>(".gallery-lightbox-img")!;
    expect(img.style.transform).toContain("scale(1)");
    expect(document.querySelector(".gallery-lightbox-scale")?.textContent).toBe("100%");

    act(() => {
      stage.dispatchEvent(
        new WheelEvent("wheel", {
          deltaY: -120,
          clientX: 200,
          clientY: 200,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(document.querySelector(".gallery-lightbox-scale")?.textContent).not.toBe("100%");
    expect(img.style.transform).toMatch(/scale\((?!1(?:\.0+)?\))/);
    expect(document.querySelector(".gallery-lightbox")?.classList.contains("is-zoomed")).toBe(
      true,
    );

    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "0", bubbles: true, cancelable: true }),
      );
    });
    expect(document.querySelector(".gallery-lightbox-scale")?.textContent).toBe("100%");

    unmount();
  });

  it("traps Tab focus inside the lightbox", () => {
    const underlying = document.createElement("button");
    underlying.textContent = "Underlying action";
    document.body.append(underlying);
    const { host, unmount } = mountGallery({ media: fullMedia });
    act(() => {
      host.querySelector<HTMLButtonElement>(".gallery-stage-image")?.click();
    });

    const close = document.querySelector<HTMLButtonElement>(".gallery-lightbox-close")!;
    expect(document.activeElement).toBe(close);
    underlying.focus();
    expect(document.activeElement).toBe(underlying);

    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
      );
    });
    expect(document.activeElement).toBe(close);

    unmount();
    underlying.remove();
  });
});
