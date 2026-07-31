// Detail-page media gallery: cover + Steam screenshots + trailers.
// List cards keep using GameMedia; this component is detail-only.

import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import type { GameMediaBlock, GameMediaScreenshot, GameMediaVideo } from "../api/types";
import { coverCandidates } from "./GameMedia";

export type GalleryImageItem = {
  kind: "cover" | "screenshot";
  id: string;
  thumbnailUrl: string;
  fullUrl: string;
  /** Ordered full-size candidates; covers retain the legacy multi-CDN fallback chain. */
  sourceCandidates: string[];
  alt: string;
};

export type GalleryVideoItem = {
  kind: "video";
  id: string;
  posterUrl: string;
  title: string;
  mp4Url: string | null;
  hlsUrl: string | null;
  dashUrl: string | null;
};

export type GalleryItem = GalleryImageItem | GalleryVideoItem;

/** Assemble gallery order: cover → highlight videos → screenshots → other videos. */
export function buildGalleryItems(input: {
  appId: number;
  name: string;
  coverUrl: string | null | undefined;
  media: GameMediaBlock | null | undefined;
}): GalleryItem[] {
  const media = input.media;
  const screenshots: GameMediaScreenshot[] = media?.screenshots ?? [];
  const videos: GameMediaVideo[] = media?.videos ?? [];

  const items: GalleryItem[] = [];
  const seenUrls = new Set<string>();

  const coverCandidatesList = coverCandidates(input.appId, input.coverUrl);
  const coverUrl = coverCandidatesList[0];
  if (coverUrl) {
    items.push({
      kind: "cover",
      id: "cover",
      thumbnailUrl: coverUrl,
      fullUrl: coverUrl,
      sourceCandidates: coverCandidatesList,
      alt: `${input.name} 封面`,
    });
    seenUrls.add(coverUrl);
  }

  const highlightVideos = videos.filter((v) => v.highlight);
  const otherVideos = videos.filter((v) => !v.highlight);

  const pushVideo = (video: GameMediaVideo) => {
    items.push({
      kind: "video",
      id: `video-${video.id}`,
      posterUrl: video.poster_url,
      title: video.title?.trim() || "预告片",
      mp4Url: video.mp4_url,
      hlsUrl: video.hls_h264_url,
      dashUrl: video.dash_h264_url,
    });
  };

  for (const video of highlightVideos) pushVideo(video);

  for (const shot of screenshots) {
    if (seenUrls.has(shot.full_url) || seenUrls.has(shot.thumbnail_url)) continue;
    seenUrls.add(shot.full_url);
    items.push({
      kind: "screenshot",
      id: `shot-${shot.id}`,
      thumbnailUrl: shot.thumbnail_url,
      fullUrl: shot.full_url,
      sourceCandidates: [shot.full_url],
      alt: `${input.name} 截图`,
    });
  }

  for (const video of otherVideos) pushVideo(video);
  return items;
}

function canPlayNativeHls(video: HTMLVideoElement | null): boolean {
  if (!video) return false;
  // Safari / some WebViews can play HLS natively.
  return video.canPlayType("application/vnd.apple.mpegurl") !== "";
}

type HlsLike = {
  loadSource: (src: string) => void;
  attachMedia: (media: HTMLMediaElement) => void;
  on: (event: string, cb: (...args: unknown[]) => void) => void;
  destroy: () => void;
};

type HlsConstructor = {
  new (config?: { enableWorker?: boolean; autoStartLoad?: boolean }): HlsLike;
  isSupported: () => boolean;
  Events: { ERROR: string };
};

async function loadHlsConstructor(): Promise<HlsConstructor | null> {
  try {
    const mod = await import("hls.js");
    return (mod.default ?? mod) as unknown as HlsConstructor;
  } catch {
    return null;
  }
}

function videoSourceIdentity(item: GalleryVideoItem): string {
  return JSON.stringify([
    item.id,
    item.posterUrl,
    item.mp4Url,
    item.hlsUrl,
    item.dashUrl,
  ]);
}

function VideoStage({
  item,
  steamUrl,
  active,
}: {
  item: GalleryVideoItem;
  steamUrl: string;
  active: boolean;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const hlsRef = useRef<HlsLike | null>(null);
  const playbackGenerationRef = useRef(0);
  const sourceIdentity = videoSourceIdentity(item);
  const sourceIdentityRef = useRef(sourceIdentity);
  sourceIdentityRef.current = sourceIdentity;
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const destroyPlayer = useCallback(() => {
    if (hlsRef.current) {
      hlsRef.current.destroy();
      hlsRef.current = null;
    }
    const el = videoRef.current;
    if (el) {
      try {
        el.pause();
      } catch {
        // jsdom does not implement media pause/load.
      }
      el.removeAttribute("src");
      try {
        el.load();
      } catch {
        // ignore unsupported media APIs in test environments
      }
    }
  }, []);

  useEffect(() => {
    if (!active) {
      playbackGenerationRef.current += 1;
      destroyPlayer();
      setPlaying(false);
      setLoading(false);
      setError(null);
    }
  }, [active, destroyPlayer]);

  useEffect(
    () => () => {
      playbackGenerationRef.current += 1;
      destroyPlayer();
    },
    [destroyPlayer],
  );

  const startPlayback = useCallback(async () => {
    if (loading) return;
    const generation = playbackGenerationRef.current + 1;
    playbackGenerationRef.current = generation;
    setError(null);
    const el = videoRef.current;
    if (!el) return;

    destroyPlayer();
    const isCurrent = () =>
      playbackGenerationRef.current === generation &&
      sourceIdentityRef.current === sourceIdentity &&
      videoRef.current === el;

    if (item.mp4Url) {
      el.src = item.mp4Url;
      setPlaying(true);
      try {
        await el.play();
      } catch {
        if (!isCurrent()) return;
        setError("当前环境无法播放预告片");
        setPlaying(false);
      }
      return;
    }

    if (item.hlsUrl && canPlayNativeHls(el)) {
      el.src = item.hlsUrl;
      setPlaying(true);
      try {
        await el.play();
      } catch {
        if (!isCurrent()) return;
        setError("当前环境无法播放预告片");
        setPlaying(false);
      }
      return;
    }

    if (item.hlsUrl) {
      setLoading(true);
      const Hls = await loadHlsConstructor();
      if (!isCurrent()) return;
      if (Hls && Hls.isSupported()) {
        const hls = new Hls({ enableWorker: true, autoStartLoad: true });
        hlsRef.current = hls;
        hls.loadSource(item.hlsUrl);
        hls.attachMedia(el);
        hls.on(Hls.Events.ERROR, (...args: unknown[]) => {
          if (!isCurrent()) return;
          const data = args[1] as { fatal?: boolean } | undefined;
          if (data?.fatal) {
            setError("当前环境无法播放预告片");
            setPlaying(false);
            setLoading(false);
            destroyPlayer();
          }
        });
        setPlaying(true);
        setLoading(false);
        try {
          await el.play();
        } catch {
          if (!isCurrent()) return;
          setError("当前环境无法播放预告片");
          setPlaying(false);
          setLoading(false);
          destroyPlayer();
        }
        return;
      }
      setLoading(false);
    }

    if (!isCurrent()) return;
    setError("当前环境无法播放预告片");
    setPlaying(false);
  }, [destroyPlayer, item.hlsUrl, item.mp4Url, loading, sourceIdentity]);

  return (
    <div className="gallery-stage-video">
      <video
        ref={videoRef}
        className="gallery-video"
        controls={playing}
        playsInline
        preload="metadata"
        poster={item.posterUrl}
        aria-label={item.title}
        onError={() => {
          if (playing) {
            setError("当前环境无法播放预告片");
            setPlaying(false);
            destroyPlayer();
          }
        }}
      />
      {!playing && (
        <button
          type="button"
          className="gallery-play-btn"
          onClick={() => void startPlayback()}
          disabled={loading}
          aria-label={`播放 ${item.title}`}
        >
          <span className="gallery-play-icon" aria-hidden="true">
            ▶
          </span>
          <span>{loading ? "正在加载…" : "播放预告片"}</span>
        </button>
      )}
      {error && (
        <div className="gallery-video-error" role="status">
          <p>{error}</p>
          <a href={steamUrl} target="_blank" rel="noreferrer noopener">
            在 Steam 查看 ↗
          </a>
        </div>
      )}
    </div>
  );
}

const LIGHTBOX_MIN_SCALE = 1;
const LIGHTBOX_MAX_SCALE = 5;
const LIGHTBOX_WHEEL_STEP = 0.12;
const LIGHTBOX_FOCUSABLE =
  "button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])";

function clampLightboxScale(value: number): number {
  return Math.min(LIGHTBOX_MAX_SCALE, Math.max(LIGHTBOX_MIN_SCALE, value));
}

/** Full-viewport image zoom overlay with wheel scale + drag pan. */
function GalleryLightbox({
  src,
  alt,
  onClose,
}: {
  src: string;
  alt: string;
  onClose: () => void;
}) {
  const lightboxRef = useRef<HTMLDivElement | null>(null);
  const closeRef = useRef<HTMLButtonElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const scaleRef = useRef(1);
  const offsetRef = useRef({ x: 0, y: 0 });
  scaleRef.current = scale;
  offsetRef.current = offset;

  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
    moved: boolean;
  } | null>(null);
  const [dragging, setDragging] = useState(false);

  const resetView = useCallback(() => {
    scaleRef.current = 1;
    offsetRef.current = { x: 0, y: 0 };
    setScale(1);
    setOffset({ x: 0, y: 0 });
  }, []);

  const applyZoom = useCallback((nextScale: number, originClientX?: number, originClientY?: number) => {
    const prevScale = scaleRef.current;
    const clamped = clampLightboxScale(nextScale);
    if (Math.abs(clamped - prevScale) < 0.001) return;

    const prevOffset = offsetRef.current;
    let nextOffset = prevOffset;
    if (clamped <= LIGHTBOX_MIN_SCALE) {
      nextOffset = { x: 0, y: 0 };
    } else {
      const stage = stageRef.current;
      if (stage != null && originClientX != null && originClientY != null && prevScale > 0) {
        const rect = stage.getBoundingClientRect();
        const cx = originClientX - rect.left - rect.width / 2;
        const cy = originClientY - rect.top - rect.height / 2;
        // Keep the point under the cursor stable while scaling.
        nextOffset = {
          x: cx - ((cx - prevOffset.x) * clamped) / prevScale,
          y: cy - ((cy - prevOffset.y) * clamped) / prevScale,
        };
      } else if (prevScale > 0) {
        nextOffset = {
          x: (prevOffset.x * clamped) / prevScale,
          y: (prevOffset.y * clamped) / prevScale,
        };
      }
    }

    scaleRef.current = clamped;
    offsetRef.current = nextOffset;
    setScale(clamped);
    setOffset(nextOffset);
  }, []);

  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    closeRef.current?.focus();
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (event.key === "Tab") {
        const lightbox = lightboxRef.current;
        if (!lightbox) return;
        const items = Array.from(
          lightbox.querySelectorAll<HTMLElement>(LIGHTBOX_FOCUSABLE),
        ).filter((element) => !(element as HTMLButtonElement).disabled);
        if (items.length === 0) {
          event.preventDefault();
          return;
        }
        const first = items[0]!;
        const last = items[items.length - 1]!;
        const active = document.activeElement;
        const inside = lightbox.contains(active);
        if (event.shiftKey && (active === first || !inside)) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && (active === last || !inside)) {
          event.preventDefault();
          first.focus();
        }
        return;
      }
      if (event.key === "+" || event.key === "=") {
        event.preventDefault();
        applyZoom(scaleRef.current + 0.25);
      } else if (event.key === "-" || event.key === "_") {
        event.preventDefault();
        applyZoom(scaleRef.current - 0.25);
      } else if (event.key === "0") {
        event.preventDefault();
        resetView();
      }
    };
    // Capture so the detail page's Escape-to-back handler never sees this key.
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      document.body.style.overflow = previousOverflow;
      previouslyFocused?.focus?.();
    };
  }, [applyZoom, resetView]);

  // Native non-passive wheel listener so preventDefault actually blocks page scroll.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const direction = event.deltaY > 0 ? -1 : 1;
      // ctrlKey trackpad pinch also maps to wheel with ctrl on some browsers.
      const intensity = event.ctrlKey ? LIGHTBOX_WHEEL_STEP * 1.6 : LIGHTBOX_WHEEL_STEP;
      applyZoom(scaleRef.current + direction * intensity, event.clientX, event.clientY);
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, [applyZoom]);

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if (scaleRef.current <= LIGHTBOX_MIN_SCALE) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const currentOffset = offsetRef.current;
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      originX: currentOffset.x,
      originY: currentOffset.y,
      moved: false,
    };
    setDragging(true);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    const next = { x: drag.originX + dx, y: drag.originY + dy };
    offsetRef.current = next;
    setOffset(next);
  };

  const endPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setDragging(false);
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // ignore if capture already released
    }
  };

  const onDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    if (scale > LIGHTBOX_MIN_SCALE) {
      resetView();
    } else {
      applyZoom(2, event.clientX, event.clientY);
    }
  };

  const percent = Math.round(scale * 100);
  const zoomed = scale > LIGHTBOX_MIN_SCALE + 0.001;

  return (
    <div
      ref={lightboxRef}
      className={`gallery-lightbox${zoomed ? " is-zoomed" : ""}${dragging ? " is-dragging" : ""}`}
      role="dialog"
      aria-modal="true"
      aria-label={alt}
      onClick={(event) => {
        // Only close on bare backdrop click when not zoomed / not after a drag.
        if (event.target === event.currentTarget && !zoomed) onClose();
      }}
    >
      <button
        ref={closeRef}
        type="button"
        className="gallery-lightbox-close"
        aria-label="关闭大图"
        onClick={onClose}
      >
        ×
      </button>
      <div className="gallery-lightbox-toolbar" aria-hidden="true">
        <span className="gallery-lightbox-scale">{percent}%</span>
        <span className="gallery-lightbox-hint">滚轮缩放 · 拖拽平移 · 双击重置 · Esc 关闭</span>
      </div>
      <div
        ref={stageRef}
        className="gallery-lightbox-stage"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endPointer}
        onPointerCancel={endPointer}
        onDoubleClick={onDoubleClick}
        onClick={(event) => {
          // Prevent backdrop close when interacting with the image stage.
          event.stopPropagation();
        }}
      >
        <img
          src={src}
          alt={alt}
          decoding="async"
          referrerPolicy="no-referrer"
          className="gallery-lightbox-img"
          draggable={false}
          style={{
            transform: `translate3d(${offset.x}px, ${offset.y}px, 0) scale(${scale})`,
          }}
        />
      </div>
      <p className="gallery-lightbox-caption">{alt}</p>
    </div>
  );
}

type GalleryMediaState = {
  failedIds: Set<string>;
  candidateIndexes: Map<string, number>;
};

type GalleryMediaAction =
  | { type: "reset" }
  | { type: "fail-item"; id: string }
  | {
      type: "fail-candidate";
      id: string;
      candidates: string[];
      failedUrl: string;
    };

const INITIAL_GALLERY_MEDIA_STATE: GalleryMediaState = {
  failedIds: new Set(),
  candidateIndexes: new Map(),
};

function galleryMediaReducer(
  state: GalleryMediaState,
  action: GalleryMediaAction,
): GalleryMediaState {
  if (action.type === "reset") {
    return {
      failedIds: new Set(),
      candidateIndexes: new Map(),
    };
  }
  if (action.type === "fail-item") {
    if (state.failedIds.has(action.id)) return state;
    const failedIds = new Set(state.failedIds);
    failedIds.add(action.id);
    return { ...state, failedIds };
  }

  const currentIndex = state.candidateIndexes.get(action.id) ?? 0;
  if (action.candidates[currentIndex] !== action.failedUrl) {
    // A second <img> may report an error for the candidate that was already advanced.
    return state;
  }
  if (currentIndex + 1 < action.candidates.length) {
    const candidateIndexes = new Map(state.candidateIndexes);
    candidateIndexes.set(action.id, currentIndex + 1);
    return { ...state, candidateIndexes };
  }
  if (state.failedIds.has(action.id)) return state;
  const failedIds = new Set(state.failedIds);
  failedIds.add(action.id);
  return { ...state, failedIds };
}

export function GameMediaGallery({
  appId,
  name,
  coverUrl,
  media,
  steamUrl,
}: {
  appId: number;
  name: string;
  coverUrl: string | null;
  media?: GameMediaBlock | null;
  steamUrl: string;
}) {
  const items = useMemo(
    () => buildGalleryItems({ appId, name, coverUrl, media }),
    [appId, name, coverUrl, media],
  );

  const [index, setIndex] = useState(0);
  const [mediaState, dispatchMedia] = useReducer(
    galleryMediaReducer,
    INITIAL_GALLERY_MEDIA_STATE,
  );
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const thumbRefs = useRef<Array<HTMLButtonElement | null>>([]);

  const visibleItems = useMemo(
    () => items.filter((item) => !mediaState.failedIds.has(item.id)),
    [items, mediaState.failedIds],
  );

  useEffect(() => {
    setIndex(0);
    dispatchMedia({ type: "reset" });
    setLightboxOpen(false);
  }, [appId, items]);

  useEffect(() => {
    if (index >= visibleItems.length) {
      setIndex(Math.max(0, visibleItems.length - 1));
    }
  }, [index, visibleItems.length]);

  const current = visibleItems[index] ?? null;

  const selectIndex = useCallback(
    (next: number) => {
      if (visibleItems.length === 0) return;
      const clamped = Math.max(0, Math.min(visibleItems.length - 1, next));
      setIndex(clamped);
      setLightboxOpen(false);
      thumbRefs.current[clamped]?.focus();
    },
    [visibleItems.length],
  );

  const markFailed = useCallback((id: string) => {
    dispatchMedia({ type: "fail-item", id });
  }, []);

  const onRailKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (visibleItems.length === 0) return;
    if (event.key === "ArrowRight") {
      event.preventDefault();
      selectIndex(index + 1);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      selectIndex(index - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      selectIndex(0);
    } else if (event.key === "End") {
      event.preventDefault();
      selectIndex(visibleItems.length - 1);
    }
  };

  if (!current) {
    // All candidates failed — letter placeholder like GameMedia.
    return (
      <div className="game-media-gallery" data-testid="game-media-gallery">
        <div className="gallery-stage">
          <div className="game-media gallery-fallback" aria-hidden="true">
            <span className="game-media-fallback">{name.slice(0, 1).toUpperCase()}</span>
          </div>
        </div>
      </div>
    );
  }

  const isImage = current.kind === "cover" || current.kind === "screenshot";
  const imageCurrent = isImage ? (current as GalleryImageItem) : null;
  const imageCurrentUrl = imageCurrent
    ? (imageCurrent.sourceCandidates[
        mediaState.candidateIndexes.get(imageCurrent.id) ?? 0
      ] ?? imageCurrent.fullUrl)
    : null;
  const videoCurrent = current.kind === "video" ? current : null;

  return (
    <div className="game-media-gallery" data-testid="game-media-gallery">
      <div className="gallery-stage">
        {imageCurrent ? (
          <button
            type="button"
            className="gallery-stage-image"
            onClick={() => setLightboxOpen(true)}
            aria-label={`放大查看：${imageCurrent.alt}`}
          >
            <img
              src={imageCurrentUrl ?? imageCurrent.fullUrl}
              alt={imageCurrent.alt}
              loading="lazy"
              decoding="async"
              referrerPolicy="no-referrer"
              onError={() =>
                dispatchMedia({
                  type: "fail-candidate",
                  id: imageCurrent.id,
                  candidates: imageCurrent.sourceCandidates,
                  failedUrl: imageCurrentUrl ?? imageCurrent.fullUrl,
                })
              }
            />
            <span className="gallery-zoom-hint" aria-hidden="true">
              点击放大
            </span>
          </button>
        ) : videoCurrent ? (
          <VideoStage
            key={videoSourceIdentity(videoCurrent)}
            item={videoCurrent}
            steamUrl={steamUrl}
            active
          />
        ) : null}
      </div>

      {visibleItems.length > 1 && (
        <div
          className="gallery-rail"
          role="listbox"
          aria-label="媒体缩略图"
          onKeyDown={onRailKeyDown}
        >
          {visibleItems.map((item, i) => {
            const selected = i === index;
            const thumbSrc =
              item.kind === "video"
                ? item.posterUrl
                : item.kind === "cover"
                  ? (item.sourceCandidates[
                      mediaState.candidateIndexes.get(item.id) ?? 0
                    ] ?? item.thumbnailUrl)
                  : item.thumbnailUrl;
            const label =
              item.kind === "video"
                ? `预告片：${item.title}`
                : item.kind === "cover"
                  ? "封面"
                  : `截图 ${i + 1}`;
            return (
              <button
                key={item.id}
                type="button"
                role="option"
                ref={(el) => {
                  thumbRefs.current[i] = el;
                }}
                className={`gallery-thumb${selected ? " is-current" : ""}${
                  item.kind === "video" ? " is-video" : ""
                }`}
                aria-label={label}
                aria-selected={selected}
                aria-current={selected ? "true" : undefined}
                onClick={() => selectIndex(i)}
              >
                <img
                  src={thumbSrc}
                  alt=""
                  loading="lazy"
                  decoding="async"
                  referrerPolicy="no-referrer"
                  onError={() => {
                    if (item.kind === "cover") {
                      dispatchMedia({
                        type: "fail-candidate",
                        id: item.id,
                        candidates: item.sourceCandidates,
                        failedUrl: thumbSrc,
                      });
                    } else {
                      markFailed(item.id);
                    }
                  }}
                />
                {item.kind === "video" && (
                  <span className="gallery-thumb-play" aria-hidden="true">
                    ▶
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}

      {lightboxOpen && imageCurrent && (
        <GalleryLightbox
          src={imageCurrentUrl ?? imageCurrent.fullUrl}
          alt={imageCurrent.alt}
          onClose={() => setLightboxOpen(false)}
        />
      )}
    </div>
  );
}
