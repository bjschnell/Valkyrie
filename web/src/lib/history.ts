// History as one timeline (#254): the away digest's sessions, with resume from
// the past-session list, and the supervisor's actions, newest first, in one list
// grouped by day. Outcome tiles count and filter it.

import type { DigestEntry, Outcome, PastSession, SupervisorEntry } from "../proto";
import { dayLabel } from "./digest";

/** How far back the timeline looks. `away` is since you were last gone. */
export type HistoryWindow = "away" | "today" | "3d" | "1w" | "month";

export const HISTORY_WINDOWS: [HistoryWindow, string][] = [
  ["away", "since you left"],
  ["today", "today"],
  ["3d", "3 days"],
  ["1w", "a week"],
  ["month", "a month"],
];

const DAY = 86_400_000;

/** Where a window starts; `null` asks the daemon for since you were last gone.
 * A month is as far back as the digest reads (31 days). */
export function historyStart(w: HistoryWindow, now: number): number | null {
  switch (w) {
    case "away":
      return null;
    case "today": {
      const d = new Date(now);
      d.setHours(0, 0, 0, 0);
      return d.getTime();
    }
    case "3d":
      return now - 3 * DAY;
    case "1w":
      return now - 7 * DAY;
    case "month":
      return now - 31 * DAY;
  }
}

/** A session's outcome as the timeline draws it. The digest's `stuck` holds a
 * live session waiting on your answer too; that one `asks` (it's in the Inbox),
 * the rest are stuck: blocked, stale, interrupted, CI red. */
export type Shown = Outcome | "asks";

export function shownOutcome(e: DigestEntry, asking: (session: number) => boolean): Shown {
  return e.outcome === "stuck" && e.session != null && asking(e.session) ? "asks" : e.outcome;
}

export function shownLabel(o: Shown): string {
  return o === "asks" ? "needs you" : o === "waiting" ? "waiting on PR/CI" : o;
}

export type TimelineEntry =
  | { kind: "session"; key: string; at: number; e: DigestEntry; outcome: Shown; past?: PastSession }
  | { kind: "supervisor"; key: string; at: number; s: SupervisorEntry };

/** The tiles over the timeline: what finished, what's stuck, what shipped. */
export type Tile = "finished" | "stuck" | "shipped";
export const TILES: [Tile, string][] = [
  ["finished", "finished"],
  ["stuck", "stuck"],
  ["shipped", "shipped"],
];

const TILE_OF: Partial<Record<Shown, Tile>> = { done: "finished", stuck: "stuck", shipped: "shipped" };

export const tileOf = (t: TimelineEntry): Tile | null => (t.kind === "session" ? (TILE_OF[t.outcome] ?? null) : null);

/**
 * One list, newest first: each digest entry once (with its past-session record,
 * to resume, when it has ended), and what the supervisor did since `since`.
 */
export function timeline(w: {
  entries: DigestEntry[];
  past: PastSession[];
  log: SupervisorEntry[];
  since: number;
  asking: (session: number) => boolean;
}): TimelineEntry[] {
  const past = new Map(w.past.map((p) => [p.key, p]));
  const out: TimelineEntry[] = w.entries.map((e) => ({
    kind: "session",
    key: `s${e.key}`,
    at: e.last_ms,
    e,
    outcome: shownOutcome(e, w.asking),
    past: e.session === undefined ? past.get(e.key) : undefined,
  }));
  w.log.forEach((s, i) => {
    if (s.at * 1000 >= w.since) out.push({ kind: "supervisor", key: `v${s.at}:${i}`, at: s.at * 1000, s });
  });
  // Stable: equal times keep sessions before the supervisor's steps about them.
  return out.sort((a, b) => b.at - a.at);
}

export function tileCounts(list: TimelineEntry[]): Record<Tile, number> {
  const n: Record<Tile, number> = { finished: 0, stuck: 0, shipped: 0 };
  for (const t of list) {
    const tile = tileOf(t);
    if (tile) n[tile]++;
  }
  return n;
}

/** The list by day, newest first, each day keeping the list's order. */
export function byDay(list: TimelineEntry[]): [string, TimelineEntry[]][] {
  const out: [string, TimelineEntry[]][] = [];
  for (const t of list) {
    const day = dayLabel(t.at);
    const last = out[out.length - 1];
    if (last && last[0] === day) last[1].push(t);
    else out.push([day, [t]]);
  }
  return out;
}

/** The order outcomes are counted in: what needs you first, then the rest as
 * the digest ranks them. */
const SHOWN_ORDER: Shown[] = ["asks", "stuck", "abandoned", "waiting", "working", "shipped", "done"];

/** `1 needs you · 2 stuck · 1 shipped`, as the timeline draws them (the digest's
 * own count calls a waiting prompt stuck). */
export function shownCounts(list: TimelineEntry[]): string {
  const n = new Map<Shown, number>();
  for (const t of list) if (t.kind === "session") n.set(t.outcome, (n.get(t.outcome) ?? 0) + 1);
  return SHOWN_ORDER.filter((o) => n.has(o))
    .map((o) => `${n.get(o)} ${shownLabel(o)}`)
    .join(" · ");
}
