import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import {
  cancelDiscoveryTask,
  getDiscoveryTaskSnapshot,
  isTauriRuntime,
  listDiscoveryTaskHistory,
  pauseDiscoveryTask,
  resumeDiscoveryTask,
  startDiscoveryTask,
} from "../../api/client";
import type { DiscoveryRunSnapshot, DiscoveryTaskRequest } from "../../types";

const DISCOVERY_TASK_EVENT = "discovery-task-updated";
const HISTORY_LIMIT = 8;

function toTimestamp(value: string | null | undefined) {
  if (!value) {
    return 0;
  }

  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? 0 : timestamp;
}

function compareSnapshots(
  left: DiscoveryRunSnapshot,
  right: DiscoveryRunSnapshot,
) {
  if (left.id !== right.id) {
    return left.id > right.id ? 1 : -1;
  }

  const leftUpdatedAt = toTimestamp(left.updatedAt);
  const rightUpdatedAt = toTimestamp(right.updatedAt);
  if (leftUpdatedAt !== rightUpdatedAt) {
    return leftUpdatedAt > rightUpdatedAt ? 1 : -1;
  }

  if (left.progressPercent !== right.progressPercent) {
    return left.progressPercent > right.progressPercent ? 1 : -1;
  }

  if (left.pagesProcessed !== right.pagesProcessed) {
    return left.pagesProcessed > right.pagesProcessed ? 1 : -1;
  }

  if (left.addedGames !== right.addedGames) {
    return left.addedGames > right.addedGames ? 1 : -1;
  }

  if (left.failedGames !== right.failedGames) {
    return left.failedGames > right.failedGames ? 1 : -1;
  }

  return 0;
}

function pickNewerSnapshot(
  current: DiscoveryRunSnapshot | null,
  incoming: DiscoveryRunSnapshot | null,
) {
  if (!incoming) {
    return current;
  }

  if (!current) {
    return incoming;
  }

  return compareSnapshots(incoming, current) >= 0 ? incoming : current;
}

function mergeHistoryEntries(
  ...snapshots: Array<DiscoveryRunSnapshot | null | undefined>
) {
  const entries = new Map<number, DiscoveryRunSnapshot>();

  for (const snapshot of snapshots) {
    if (!snapshot) {
      continue;
    }

    const current = entries.get(snapshot.id);
    if (!current || compareSnapshots(snapshot, current) >= 0) {
      entries.set(snapshot.id, snapshot);
    }
  }

  return [...entries.values()]
    .sort((left, right) => compareSnapshots(right, left))
    .slice(0, HISTORY_LIMIT);
}

function mergeHistory(
  currentHistory: DiscoveryRunSnapshot[],
  incomingHistory: DiscoveryRunSnapshot[],
  currentSnapshot: DiscoveryRunSnapshot | null,
  incomingSnapshot: DiscoveryRunSnapshot | null,
) {
  return mergeHistoryEntries(
    currentSnapshot,
    incomingSnapshot,
    ...currentHistory,
    ...incomingHistory,
  );
}

export function useDiscoveryTask() {
  const [snapshot, setSnapshot] = useState<DiscoveryRunSnapshot | null>(null);
  const [history, setHistory] = useState<DiscoveryRunSnapshot[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const snapshotRef = useRef<DiscoveryRunSnapshot | null>(null);
  const historyRef = useRef<DiscoveryRunSnapshot[]>([]);

  function applySnapshot(nextSnapshot: DiscoveryRunSnapshot) {
    const resolvedSnapshot = pickNewerSnapshot(snapshotRef.current, nextSnapshot);
    const resolvedHistory = mergeHistory(
      historyRef.current,
      [],
      snapshotRef.current,
      nextSnapshot,
    );

    snapshotRef.current = resolvedSnapshot;
    historyRef.current = resolvedHistory;
    setSnapshot(resolvedSnapshot);
    setHistory(resolvedHistory);
  }

  async function refresh() {
    setIsLoading(true);
    try {
      const [nextSnapshot, nextHistory] = await Promise.all([
        getDiscoveryTaskSnapshot(),
        listDiscoveryTaskHistory(HISTORY_LIMIT),
      ]);
      const resolvedSnapshot = pickNewerSnapshot(
        snapshotRef.current,
        nextSnapshot,
      );
      const resolvedHistory = mergeHistory(
        historyRef.current,
        nextHistory,
        snapshotRef.current,
        nextSnapshot,
      );

      snapshotRef.current = resolvedSnapshot;
      historyRef.current = resolvedHistory;
      setSnapshot(resolvedSnapshot);
      setHistory(resolvedHistory);

      return { snapshot: resolvedSnapshot, history: nextHistory };
    } finally {
      setIsLoading(false);
    }
  }

  async function runAction(
    action: () => Promise<DiscoveryRunSnapshot>,
  ): Promise<DiscoveryRunSnapshot> {
    setIsLoading(true);
    try {
      const nextSnapshot = await action();
      applySnapshot(nextSnapshot);
      return nextSnapshot;
    } finally {
      setIsLoading(false);
    }
  }

  async function start(request: DiscoveryTaskRequest) {
    return runAction(() => startDiscoveryTask(request));
  }

  async function pause() {
    return runAction(() => pauseDiscoveryTask());
  }

  async function resume() {
    return runAction(() => resumeDiscoveryTask());
  }

  async function cancel() {
    return runAction(() => cancelDiscoveryTask());
  }

  useEffect(() => {
    void refresh();

    if (!isTauriRuntime()) {
      return;
    }

    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    void listen<DiscoveryRunSnapshot>(
      DISCOVERY_TASK_EVENT,
      ({ payload }) => {
        if (isDisposed) {
          return;
        }
        applySnapshot(payload);
      },
    ).then((cleanup) => {
      if (isDisposed) {
        cleanup();
        return;
      }
      unlisten = cleanup;
    });

    return () => {
      isDisposed = true;
      unlisten?.();
    };
  }, []);

  return {
    snapshot,
    history,
    isLoading,
    refresh,
    start,
    pause,
    resume,
    cancel,
  };
}
