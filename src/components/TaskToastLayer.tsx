import { useEffect, useRef, useState } from "react";

export type TaskToastTone = "success" | "warning" | "danger";

export interface TaskToastItem {
  id: number;
  tone: TaskToastTone;
  title: string;
  message: string;
}

function TaskToast({
  toast,
  onDismiss,
}: {
  toast: TaskToastItem;
  onDismiss: (id: number) => void;
}) {
  const [hoverReady, setHoverReady] = useState(false);
  const hoverTimerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (hoverTimerRef.current != null) {
        window.clearTimeout(hoverTimerRef.current);
      }
    };
  }, []);

  function clearHoverTimer() {
    if (hoverTimerRef.current != null) {
      window.clearTimeout(hoverTimerRef.current);
      hoverTimerRef.current = null;
    }
  }

  function handleMouseEnter() {
    if (hoverReady || hoverTimerRef.current != null) {
      return;
    }

    hoverTimerRef.current = window.setTimeout(() => {
      setHoverReady(true);
      hoverTimerRef.current = null;
    }, 3_000);
  }

  function handleMouseLeave() {
    clearHoverTimer();
    if (hoverReady) {
      onDismiss(toast.id);
    }
  }

  return (
    <button
      type="button"
      className={`task-toast task-toast-${toast.tone}`}
      onClick={() => onDismiss(toast.id)}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      <div className="task-toast-head">
        <strong>{toast.title}</strong>
        <span aria-hidden="true">×</span>
      </div>
      <p>{toast.message}</p>
      <small>点击关闭，或悬停 3 秒后移开自动关闭</small>
    </button>
  );
}

export function TaskToastLayer({
  toasts,
  onDismiss,
}: {
  toasts: TaskToastItem[];
  onDismiss: (id: number) => void;
}) {
  if (toasts.length === 0) {
    return null;
  }

  return (
    <div className="task-toast-layer" aria-live="polite">
      {toasts.map((toast) => (
        <TaskToast key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}
