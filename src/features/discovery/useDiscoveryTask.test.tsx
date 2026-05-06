// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveryRunSnapshot } from "../../types";
import {
  getDiscoveryTaskSnapshot,
  isTauriRuntime,
  listDiscoveryTaskHistory,
  startDiscoveryTask,
} from "../../api/client";
import { useDiscoveryTask } from "./useDiscoveryTask";

const listenMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

vi.mock("../../api/client", async () => {
  const actual = await vi.importActual("../../api/client");

  return {
    ...actual,
    isTauriRuntime: vi.fn(),
    getDiscoveryTaskSnapshot: vi.fn(),
    listDiscoveryTaskHistory: vi.fn(),
    startDiscoveryTask: vi.fn(),
    pauseDiscoveryTask: vi.fn(),
    resumeDiscoveryTask: vi.fn(),
    cancelDiscoveryTask: vi.fn(),
  };
});

const tauriRuntimeMock = vi.mocked(isTauriRuntime);
const getSnapshotMock = vi.mocked(getDiscoveryTaskSnapshot);
const listHistoryMock = vi.mocked(listDiscoveryTaskHistory);
const startTaskMock = vi.mocked(startDiscoveryTask);

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;

  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });

  return { promise, resolve, reject };
}

function buildSnapshot(
  overrides: Partial<DiscoveryRunSnapshot> = {},
): DiscoveryRunSnapshot {
  return {
    id: 1,
    status: "running",
    completionReason: null,
    syncMode: "full",
    targetAddedGames: 10,
    pageSize: 25,
    pagesProcessed: 2,
    scannedApps: 50,
    addedGames: 3,
    addedNewGames: 2,
    addedClassicGames: 1,
    skippedExisting: 40,
    skippedNonMultiplayer: 6,
    failedGames: 1,
    currentAppid: 123,
    lastAppid: 120,
    haveMoreResults: true,
    startedAt: "2026-04-27T00:00:00Z",
    updatedAt: "2026-04-27T00:05:00Z",
    finishedAt: null,
    lastError: null,
    failures: [],
    progressPercent: 30,
    ...overrides,
  };
}

describe("useDiscoveryTask", () => {
  beforeEach(() => {
    listenMock.mockReset();
    tauriRuntimeMock.mockReset();
    getSnapshotMock.mockReset();
    listHistoryMock.mockReset();
    startTaskMock.mockReset();
  });

  it("hydrates snapshot and history from client commands on mount", async () => {
    const snapshot = buildSnapshot();
    const history = [
      buildSnapshot({ id: 2, status: "completed", progressPercent: 100 }),
      buildSnapshot({ id: 1, status: "running", progressPercent: 30 }),
    ];

    tauriRuntimeMock.mockReturnValue(true);
    getSnapshotMock.mockResolvedValue(snapshot);
    listHistoryMock.mockResolvedValue(history);
    listenMock.mockResolvedValue(() => {});

    const { result } = renderHook(() => useDiscoveryTask());

    expect(result.current.isLoading).toBe(true);

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    expect(getSnapshotMock).toHaveBeenCalledTimes(1);
    expect(listHistoryMock).toHaveBeenCalledWith(8);
    expect(result.current.snapshot).toEqual(snapshot);
    expect(result.current.history).toEqual(history);
  });

  it("replaces the snapshot when a discovery update event arrives", async () => {
    const initialSnapshot = buildSnapshot();
    const nextSnapshot = buildSnapshot({
      status: "paused",
      currentAppid: null,
      updatedAt: "2026-04-27T00:06:00Z",
      progressPercent: 40,
    });

    tauriRuntimeMock.mockReturnValue(true);
    getSnapshotMock.mockResolvedValue(initialSnapshot);
    listHistoryMock.mockResolvedValue([initialSnapshot]);

    let eventHandler:
      | ((event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>)
      | undefined;

    listenMock.mockImplementation(
      async (
        eventName: string,
        handler: (event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>,
      ) => {
        expect(eventName).toBe("discovery-task-updated");
        eventHandler = handler;
        return () => {};
      },
    );

    const { result } = renderHook(() => useDiscoveryTask());

    await waitFor(() => expect(result.current.snapshot).toEqual(initialSnapshot));

    await act(async () => {
      await eventHandler?.({ payload: nextSnapshot });
    });

    expect(result.current.snapshot).toEqual(nextSnapshot);
  });

  it("keeps newer event state when hydration resolves later with an older snapshot", async () => {
    const hydrationSnapshot = buildSnapshot({
      status: "running",
      updatedAt: "2026-04-27T00:05:00Z",
      progressPercent: 30,
    });
    const liveSnapshot = buildSnapshot({
      status: "paused",
      currentAppid: null,
      updatedAt: "2026-04-27T00:06:00Z",
      progressPercent: 40,
    });
    const hydration = createDeferred<DiscoveryRunSnapshot | null>();
    const history = createDeferred<DiscoveryRunSnapshot[]>();

    tauriRuntimeMock.mockReturnValue(true);
    getSnapshotMock.mockReturnValue(hydration.promise);
    listHistoryMock.mockReturnValue(history.promise);

    let eventHandler:
      | ((event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>)
      | undefined;

    listenMock.mockImplementation(
      async (
        _eventName: string,
        handler: (event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>,
      ) => {
        eventHandler = handler;
        return () => {};
      },
    );

    const { result } = renderHook(() => useDiscoveryTask());

    await act(async () => {
      await Promise.resolve();
      await eventHandler?.({ payload: liveSnapshot });
    });

    expect(result.current.snapshot).toEqual(liveSnapshot);
    expect(result.current.history).toEqual([liveSnapshot]);

    await act(async () => {
      history.resolve([hydrationSnapshot]);
      hydration.resolve(hydrationSnapshot);
      await Promise.all([history.promise, hydration.promise]);
    });

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    expect(result.current.snapshot).toEqual(liveSnapshot);
    expect(result.current.history).toEqual([liveSnapshot]);
  });

  it("keeps newer event state when an action resolves later with an older snapshot", async () => {
    const initialSnapshot = buildSnapshot({
      status: "paused",
      currentAppid: null,
      updatedAt: "2026-04-27T00:05:00Z",
      progressPercent: 30,
    });
    const liveSnapshot = buildSnapshot({
      status: "running",
      currentAppid: 456,
      updatedAt: "2026-04-27T00:06:00Z",
      progressPercent: 40,
    });
    const staleActionSnapshot = buildSnapshot({
      status: "running",
      currentAppid: 123,
      updatedAt: "2026-04-27T00:05:30Z",
      progressPercent: 35,
    });
    const action = createDeferred<DiscoveryRunSnapshot>();

    tauriRuntimeMock.mockReturnValue(true);
    getSnapshotMock.mockResolvedValue(initialSnapshot);
    listHistoryMock.mockResolvedValue([initialSnapshot]);
    startTaskMock.mockReturnValue(action.promise);

    let eventHandler:
      | ((event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>)
      | undefined;

    listenMock.mockImplementation(
      async (
        _eventName: string,
        handler: (event: { payload: DiscoveryRunSnapshot }) => void | Promise<void>,
      ) => {
        eventHandler = handler;
        return () => {};
      },
    );

    const { result } = renderHook(() => useDiscoveryTask());

    await waitFor(() => expect(result.current.snapshot).toEqual(initialSnapshot));

    let actionResult: DiscoveryRunSnapshot | undefined;
    await act(async () => {
      const pendingAction = result.current.start({
        syncMode: "full",
        targetAddedGames: 10,
        pageSize: 25,
      }).then((value) => {
        actionResult = value;
      });

      await Promise.resolve();
      await eventHandler?.({ payload: liveSnapshot });
      action.resolve(staleActionSnapshot);
      await pendingAction;
    });

    expect(actionResult).toEqual(staleActionSnapshot);
    expect(result.current.snapshot).toEqual(liveSnapshot);
    expect(result.current.history).toEqual([liveSnapshot]);
  });
});
