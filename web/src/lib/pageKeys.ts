// A page's plain keys, bound once (#191). `actions` must have exactly the keys
// lib/shortcuts.ts lists for the page, so the '?' sheet can't drift from what
// the page does: tsc rejects a key bound but not listed, or listed but not bound.

import { useEffect, useRef } from "react";
import { PAGE_KEYS, type KeyedPage } from "../lib/shortcuts";

export function usePageKeys<P extends KeyedPage>(
  page: P,
  actions: Record<keyof (typeof PAGE_KEYS)[P], () => void>,
  opts: { off?: boolean; blurOnEscape?: boolean } = {},
) {
  void page; // names the table `actions` is checked against
  // The latest closures, so the listener is added once.
  const latest = useRef({ actions, opts });
  latest.current = { actions, opts };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const { actions, opts } = latest.current;
      if (opts.off || e.ctrlKey || e.metaKey || e.altKey) return;
      if ((e.target as HTMLElement).closest("textarea, input, select, [contenteditable]")) {
        if (opts.blurOnEscape && e.key === "Escape") (e.target as HTMLElement).blur();
        return;
      }
      const action = (actions as Record<string, (() => void) | undefined>)[e.key];
      if (action) {
        e.preventDefault();
        action();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
