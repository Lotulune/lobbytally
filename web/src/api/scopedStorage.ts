// Per-service storage isolation (PRD_CS CS-007).
//
// Every piece of per-service state — session tokens, ETag cache snapshots,
// the pending feedback queue, play-intent overrides, unsynced preference
// patches — lives under a key prefix derived from the normalized service
// origin. Switching services switches the prefix, so an old service's token
// can never be read (or sent) in the new service's namespace.

import { serviceNamespacePrefix } from "./serverOrigin";
import type { StorageLike } from "./types";

/**
 * StorageLike wrapper that transparently prefixes all keys with the service
 * namespace. Enumeration (`length`/`key`) exposes the *unprefixed* view so
 * callers like ApiClient.clearCachedResponses keep working unchanged.
 */
export class ScopedStorage implements StorageLike {
  private readonly base: StorageLike;
  private readonly prefix: string;

  constructor(base: StorageLike, origin: string) {
    this.base = base;
    this.prefix = serviceNamespacePrefix(origin);
  }

  /** Names of all keys inside this namespace, without the prefix. */
  private visibleKeys(): string[] {
    const store = this.base as Partial<Storage>;
    if (typeof store.length !== "number" || typeof store.key !== "function") return [];
    const keys: string[] = [];
    for (let i = 0; i < store.length; i += 1) {
      const key = store.key(i);
      if (key && key.startsWith(this.prefix)) keys.push(key.slice(this.prefix.length));
    }
    return keys;
  }

  get length(): number {
    return this.visibleKeys().length;
  }

  key(index: number): string | null {
    return this.visibleKeys()[index] ?? null;
  }

  getItem(key: string): string | null {
    return this.base.getItem(this.prefix + key);
  }

  setItem(key: string, value: string): void {
    this.base.setItem(this.prefix + key, value);
  }

  removeItem(key: string): void {
    this.base.removeItem(this.prefix + key);
  }
}

/**
 * Delete every key belonging to a service namespace (CS-009 "删除指定服务的
 * 本地数据"). Returns the number of removed keys. The service registry entry
 * is handled separately by the caller.
 */
export function deleteServiceNamespace(base: StorageLike, origin: string): number {
  const store = base as Partial<Storage>;
  if (typeof store.length !== "number" || typeof store.key !== "function") return 0;
  const prefix = serviceNamespacePrefix(origin);
  const keys: string[] = [];
  for (let i = 0; i < store.length; i += 1) {
    const key = store.key(i);
    if (key && key.startsWith(prefix)) keys.push(key);
  }
  for (const key of keys) base.removeItem(key);
  return keys.length;
}
