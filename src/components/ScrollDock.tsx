import { useEffect, useRef, useState } from "react";

const SCROLL_DOCK_IDLE_MS = 2_500;
const SCROLL_DELTA_THRESHOLD = 14;

type ScrollDirection = "up" | "down";

function hasIndependentScrollContainer(container: HTMLElement | null) {
  if (container == null) {
    return false;
  }

  const overflowY = window.getComputedStyle(container).overflowY;
  return overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay";
}

function readScrollMetrics(container: HTMLElement | null) {
  if (container != null && hasIndependentScrollContainer(container)) {
    const maxScroll = Math.max(0, container.scrollHeight - container.clientHeight);
    const top = container.scrollTop;
    const progress = maxScroll > 0 ? Math.round((top / maxScroll) * 100) : 0;
    return { top, progress, maxScroll };
  }

  const root = document.documentElement;
  const body = document.body;
  const top = window.scrollY || root.scrollTop || body.scrollTop || 0;
  const viewportHeight = window.innerHeight || root.clientHeight || 0;
  const scrollHeight = Math.max(
    body.scrollHeight,
    root.scrollHeight,
    body.offsetHeight,
    root.offsetHeight,
    body.clientHeight,
    root.clientHeight,
  );
  const maxScroll = Math.max(0, scrollHeight - viewportHeight);
  const progress = maxScroll > 0 ? Math.round((top / maxScroll) * 100) : 0;
  return { top, progress, maxScroll };
}

function scrollToTarget(container: HTMLElement | null, top: number) {
  const safeTop = Math.max(0, top);

  if (container != null && hasIndependentScrollContainer(container)) {
    container.scrollTo({ top: safeTop, behavior: "smooth" });
    return;
  }

  window.scrollTo({ top: safeTop, behavior: "smooth" });
}

export function ScrollDock({
  scrollContainer,
}: {
  scrollContainer: HTMLElement | null;
}) {
  const [visible, setVisible] = useState(false);
  const [progress, setProgress] = useState(0);
  const [direction, setDirection] = useState<ScrollDirection>("down");
  const idleTimerRef = useRef<number | null>(null);
  const lastTopRef = useRef(0);

  useEffect(() => {
    const syncInitialMetrics = () => {
      const metrics = readScrollMetrics(scrollContainer);
      lastTopRef.current = metrics.top;
      setProgress(metrics.progress);
    };

    syncInitialMetrics();
  }, [scrollContainer]);

  useEffect(() => {
    const target = scrollContainer != null && hasIndependentScrollContainer(scrollContainer)
      ? scrollContainer
      : window;

    function clearIdleTimer() {
      if (idleTimerRef.current != null) {
        window.clearTimeout(idleTimerRef.current);
        idleTimerRef.current = null;
      }
    }

    function scheduleHide() {
      clearIdleTimer();
      idleTimerRef.current = window.setTimeout(() => {
        setVisible(false);
        idleTimerRef.current = null;
      }, SCROLL_DOCK_IDLE_MS);
    }

    function handleScroll() {
      const metrics = readScrollMetrics(scrollContainer);
      const delta = metrics.top - lastTopRef.current;

      lastTopRef.current = metrics.top;
      setProgress(metrics.progress);

      if (Math.abs(delta) >= SCROLL_DELTA_THRESHOLD) {
        setDirection(delta < 0 ? "up" : "down");
      }

      if (metrics.maxScroll <= 0) {
        setVisible(false);
        clearIdleTimer();
        return;
      }

      setVisible(true);
      scheduleHide();
    }

    target.addEventListener("scroll", handleScroll, { passive: true });
    return () => {
      target.removeEventListener("scroll", handleScroll);
      clearIdleTimer();
    };
  }, [scrollContainer]);

  const metrics = readScrollMetrics(scrollContainer);
  const actionLabel = direction === "up" ? "置顶" : "置底";
  const actionSymbol = direction === "up" ? "↑" : "↓";

  if (metrics.maxScroll <= 0) {
    return null;
  }

  return (
    <div
      aria-hidden={!visible}
      className={`scroll-dock${visible ? " is-visible" : ""}`}
    >
      <div className="scroll-dock-panel">
        <div className="scroll-progress-inline">
          <span>{progress}%</span>
          <div
            aria-hidden="true"
            className="scroll-progress-bar"
            style={{ ["--scroll-progress" as string]: `${progress}%` }}
          />
        </div>
        <button
          className="scroll-jump-button"
          onClick={() =>
            scrollToTarget(scrollContainer, direction === "up" ? 0 : metrics.maxScroll)
          }
          tabIndex={visible ? 0 : -1}
          type="button"
        >
          <strong>{actionSymbol}</strong>
          <span>{actionLabel}</span>
        </button>
      </div>
    </div>
  );
}
