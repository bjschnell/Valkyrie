// #104 trial ("Instant pages"): what each page last loaded, kept for the next
// visit so it paints at once and refreshes behind it, instead of "Loading…".

import { useCallback, useEffect, useRef, useState } from "react";
import { trialOn } from "./lib/trial";
import { useApp } from "./store";

const kept = new Map<string, unknown>();
const going = new Map<string, Promise<unknown>>();

/** Loads `key` (one request at a time per key) and keeps what came back. A
 * loader returns null when the daemon couldn't be asked, or throws its reason. */
function load<T>(key: string, get: () => Promise<T | null>): Promise<T | null> {
  const already = going.get(key);
  if (already) return already as Promise<T | null>;
  const p = get()
    .then((v) => {
      if (v !== null) kept.set(key, v);
      return v;
    })
    .finally(() => going.delete(key));
  going.set(key, p);
  return p;
}

/** Starts loading `key` ahead of a visit (a hover or a press on a link to it). */
export function prefetch<T>(key: string, get: () => Promise<T | null>): void {
  if (!trialOn("cache") || useApp.getState().status !== "open") return;
  void load(key, get).catch(() => undefined);
}

export interface Cached<T> {
  /** The last data, possibly from an earlier visit; null until there's some. */
  data: T | null;
  /** Why the latest load failed; null when it didn't. */
  failed: string | null;
  /** Loads again, as after a change on the page. */
  reload: () => Promise<void>;
}

/** The data at `key`: what an earlier visit kept at once, then the daemon's. */
export function useCached<T>(key: string, get: () => Promise<T | null>, fresh?: (v: T) => void): Cached<T> {
  const status = useApp((s) => s.status);
  const seed = () => (trialOn("cache") ? ((kept.get(key) as T | undefined) ?? null) : null);
  const [data, setData] = useState<{ key: string; v: T | null }>(() => ({ key, v: seed() }));
  const [failed, setFailed] = useState<string | null>(null);
  const getRef = useRef(get);
  getRef.current = get;
  const freshRef = useRef(fresh);
  freshRef.current = fresh;
  const keyRef = useRef(key);
  keyRef.current = key;

  const reload = useCallback(async () => {
    const at = keyRef.current;
    try {
      const v = await load(at, getRef.current);
      if (keyRef.current !== at) return;
      if (v === null) {
        setFailed("couldn't ask the daemon");
        return;
      }
      setData({ key: at, v });
      setFailed(null);
      freshRef.current?.(v);
    } catch (e) {
      if (keyRef.current === at) setFailed((e as Error).message);
    }
  }, []);

  useEffect(() => {
    if (status === "open") void reload();
  }, [status, key, reload]);

  // A new key shows what it kept, never the last key's data.
  const v = data.key === key ? data.v : seed();
  return { data: v, failed, reload };
}
