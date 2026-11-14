// Pages paint what they last loaded at once and refresh behind it, instead of
// showing "Loading…" on every visit (#104). The keeping itself is lib/kept.ts.

import { useCallback, useEffect, useRef, useState } from "react";
import { Kept, type Loader } from "./lib/kept";
import { useApp } from "./store";

const kept = new Kept();

/** Starts loading `key` ahead of a visit (a hover or a press on a link to it). */
export function prefetch<T>(key: string, get: Loader<T>): void {
  if (useApp.getState().status !== "open") return;
  void kept.load(key, get).catch(() => undefined);
}

export interface Cached<T> {
  /** The last data, possibly from an earlier visit; null until there's some. */
  data: T | null;
  /** Why the latest load failed; null when it didn't. */
  failed: string | null;
  /** Loads again, as after a change on the page. */
  reload: () => Promise<void>;
}

/** The data at `key`: what an earlier visit kept at once, then the daemon's.
 * `fresh` runs with each answer the daemon gives. */
export function useCached<T>(key: string, get: Loader<T>, fresh?: (v: T) => void): Cached<T> {
  const status = useApp((s) => s.status);
  const [data, setData] = useState<{ key: string; v: T | null }>(() => ({ key, v: kept.peek<T>(key) }));
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
      const v = await kept.load(at, getRef.current);
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
  return { data: data.key === key ? data.v : kept.peek<T>(key), failed, reload };
}
