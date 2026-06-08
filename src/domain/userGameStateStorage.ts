import type { UserGameState, UserGameStatePatch } from "../types";

export function getStoredUserGameState(
  serviceInstanceId: string,
  appid: number,
): UserGameState {
  return {
    ...emptyUserState(),
    ...readServiceUserState(serviceInstanceId)[appid],
  };
}

export function setStoredUserGameState(
  serviceInstanceId: string,
  appid: number,
  patch: UserGameStatePatch,
): UserGameState {
  const stateByAppid = readServiceUserState(serviceInstanceId);
  const nextState: UserGameState = {
    ...emptyUserState(),
    ...stateByAppid[appid],
    ...patch,
    updatedAt: new Date().toISOString(),
  };

  writeServiceUserState(serviceInstanceId, {
    ...stateByAppid,
    [appid]: nextState,
  });

  return nextState;
}

export function clearStoredUserGameStates(serviceInstanceId: string) {
  getStorage()?.removeItem(storageKey(serviceInstanceId));
}

function readServiceUserState(serviceInstanceId: string): Record<number, UserGameState> {
  const storage = getStorage();
  if (!storage) {
    return {};
  }

  const rawValue = storage.getItem(storageKey(serviceInstanceId));
  if (!rawValue) {
    return {};
  }

  try {
    const parsed = JSON.parse(rawValue);
    if (!parsed || typeof parsed !== "object") {
      return {};
    }

    const states: Record<number, UserGameState> = {};
    for (const [appid, value] of Object.entries(parsed)) {
      const numericAppid = Number(appid);
      if (Number.isInteger(numericAppid) && isUserGameState(value)) {
        states[numericAppid] = value;
      }
    }
    return states;
  } catch {
    return {};
  }
}

function writeServiceUserState(
  serviceInstanceId: string,
  stateByAppid: Record<number, UserGameState>,
) {
  getStorage()?.setItem(storageKey(serviceInstanceId), JSON.stringify(stateByAppid));
}

function storageKey(serviceInstanceId: string) {
  return `mpgs.userGameState.v1.${serviceInstanceId}`;
}

function getStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }

  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function emptyUserState(): UserGameState {
  return {
    favorite: false,
    wishlist: false,
    followed: false,
    viewed: false,
    updatedAt: null,
  };
}

function isUserGameState(value: unknown): value is UserGameState {
  if (!value || typeof value !== "object") {
    return false;
  }

  const candidate = value as Partial<UserGameState>;
  return (
    typeof candidate.favorite === "boolean" &&
    typeof candidate.wishlist === "boolean" &&
    typeof candidate.followed === "boolean" &&
    typeof candidate.viewed === "boolean" &&
    (candidate.updatedAt === null ||
      candidate.updatedAt === undefined ||
      typeof candidate.updatedAt === "string")
  );
}
