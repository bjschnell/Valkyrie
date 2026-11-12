// The away digest (backlog #105): grouping and labels for the web view. The
// daemon sorts the entries and writes the markdown to copy.

import type { DigestEntry, DigestGroup, Outcome } from "../proto";

export const OUTCOMES: Outcome[] = ["stuck", "abandoned", "waiting", "working", "shipped", "done"];

export function outcomeLabel(o: Outcome): string {
  return o === "waiting" ? "waiting on PR/CI" : o;
}

/** `Tue 10 Nov`, in the viewer's time zone. */
export function dayLabel(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
}

function dayStart(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** `09:12`. */
export function clockOf(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** `09:12–11:40` within a day; across midnight, both days. */
export function spanOf(e: DigestEntry): string {
  if (dayStart(e.first_ms) !== dayStart(e.last_ms)) {
    return `${dayLabel(e.first_ms)} ${clockOf(e.first_ms)} – ${dayLabel(e.last_ms)} ${clockOf(e.last_ms)}`;
  }
  const [a, b] = [clockOf(e.first_ms), clockOf(e.last_ms)];
  return a === b ? a : `${a}–${b}`;
}

/** Entries by repository (in the daemon's order) or by day (newest first),
 * each group keeping the entries' own order. */
export function groupDigest(entries: DigestEntry[], by: DigestGroup): [string, DigestEntry[]][] {
  const keyed = entries.map((e) => ({ e, day: dayStart(e.last_ms) }));
  if (by === "day") keyed.sort((a, b) => b.day - a.day);
  const groups: [string, DigestEntry[]][] = [];
  for (const { e, day } of keyed) {
    const key = by === "repo" ? e.repo : dayLabel(day);
    const at = groups.find(([k]) => k === key);
    if (at) at[1].push(e);
    else groups.push([key, [e]]);
  }
  return groups;
}

/** `2 stuck · 1 shipped`, most urgent first. */
export function counts(entries: DigestEntry[]): string {
  return OUTCOMES.map((o) => [o, entries.filter((e) => e.outcome === o).length] as const)
    .filter(([, n]) => n > 0)
    .map(([o, n]) => `${n} ${outcomeLabel(o)}`)
    .join(" · ");
}

/** Where a pick of the window starts: `null` is since you were last gone. */
export type Window = "away" | "12h" | "today" | "3d";

export function windowStart(w: Window, now: number): number | null {
  switch (w) {
    case "away":
      return null;
    case "12h":
      return now - 12 * 3_600_000;
    case "today":
      return dayStart(now);
    case "3d":
      return now - 3 * 86_400_000;
  }
}
