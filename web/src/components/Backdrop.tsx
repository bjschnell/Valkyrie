import { useEffect, useRef, useState } from "react";
import { hasRain, packOf, type Look } from "../lib/theme";
import { useApp } from "../store";
import { useAttention } from "./Attention";
import { Rain } from "./Rain";

/**
 * What a pack's module is told: how many things wait on you, and how many
 * sessions are working and since when. A built-in look never sees this; a pack's
 * avatar may follow it (#191).
 */
export interface PackState {
  needs: number;
  working: number;
  /** When the longest-running working session started, ms since the epoch. */
  workingSince: number | null;
}

/** A pack's module (`index.js` in its web folder). */
interface PackModule {
  mount(
    el: HTMLElement,
    ctx: { base: string; glyphs: string | null },
  ): { update(state: PackState): void; destroy(): void } | Promise<{ update(state: PackState): void; destroy(): void }>;
}

/** Behind the page: a pack's module when its look has one, else the rain of a look with rain. */
export function Backdrop({ look }: { look: Look }) {
  const pack = packOf(look);
  const [failed, setFailed] = useState(false);
  if (pack?.base && !failed) return <PackLayer key={look} base={pack.base} glyphs={pack.glyphs} onFail={() => setFailed(true)} />;
  return hasRain(look) ? <Rain key={look} glyphs={pack?.glyphs ?? undefined} /> : null;
}

function usePackState(): PackState {
  const needs = useAttention().length;
  const sessions = useApp((s) => s.sessions);
  const working = sessions.filter((s) => s.exited === null && s.status.state === "working");
  const since = working.map((s) => s.status.since_ms);
  return { needs, working: working.length, workingSince: since.length ? Math.min(...since) : null };
}

function PackLayer({ base, glyphs, onFail }: { base: string; glyphs: string | null; onFail: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const handle = useRef<{ update(state: PackState): void; destroy(): void } | null>(null);
  const state = usePackState();
  const latest = useRef(state);
  latest.current = state;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let gone = false;
    (async () => {
      const mod = (await import(/* @vite-ignore */ `${base}index.js`)) as PackModule;
      const h = await mod.mount(el, { base, glyphs });
      if (gone) return h.destroy();
      handle.current = h;
      h.update(latest.current);
    })().catch((e) => {
      console.warn("[valk] theme pack module failed, falling back to rain:", e);
      if (!gone) onFail();
    });
    return () => {
      gone = true;
      handle.current?.destroy();
      handle.current = null;
    };
    // The module is mounted once per pack; state goes to it through `update`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base]);

  useEffect(() => {
    handle.current?.update(state);
  }, [state.needs, state.working, state.workingSince]);

  // Fixed like the rain, from its page's left edge to the right of the screen;
  // but beside an open session on a wide screen, only over the list's column,
  // so what the pack draws isn't centered under the opaque pane.
  useEffect(() => {
    const el = ref.current;
    const page = el?.parentElement;
    if (!el || !page) return;
    const fit = () => {
      const r = page.getBoundingClientRect();
      const pane = document.querySelector(".split-pane");
      const open = pane && !pane.querySelector(".split-empty");
      const right = open ? r.right : innerWidth;
      Object.assign(el.style, { left: `${r.left}px`, top: "0px", width: `${right - r.left}px`, height: `${innerHeight}px` });
    };
    // After the route's page has drawn.
    const later = () => requestAnimationFrame(fit);
    fit();
    const watch = new ResizeObserver(fit);
    watch.observe(page);
    window.addEventListener("resize", fit);
    window.addEventListener("hashchange", later);
    return () => {
      watch.disconnect();
      window.removeEventListener("resize", fit);
      window.removeEventListener("hashchange", later);
    };
  }, []);

  return <div ref={ref} className="rain pack-layer" aria-hidden="true" />;
}
