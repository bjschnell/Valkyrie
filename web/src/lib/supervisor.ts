// The supervisor page (backlog #180). What it waits on you for is in the Inbox's
// attention queue (lib/attention.ts, #251).
import { age } from "./time";

/** The Watching section's line: `2 PRs · read 2m ago · next in 3m`. `nowMs` in
 * ms, the poll's times in seconds. */
export function watchLine(w: { prs: unknown[]; polled_at?: number; next_at?: number; on: boolean }, nowMs: number): string {
  if (!w.on) return "off: nothing is read";
  if (w.polled_at === undefined) return "the poll hasn't run yet";
  const n = `${w.prs.length} PR${w.prs.length === 1 ? "" : "s"}`;
  const read = `read ${age(w.polled_at * 1000, nowMs)} ago`;
  const next = w.next_at !== undefined && w.next_at * 1000 > nowMs ? ` · next in ${age(nowMs, w.next_at * 1000)}` : "";
  return `${n} · ${read}${next}`;
}
