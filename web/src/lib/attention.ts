// One attention queue (#251): everything that waits on the user is an item in the
// Inbox's Needs you group, and the badge, tab title and app badge count them all.
// Sessions are round dots (Ask, Stuck, Done); records are squares (Disk,
// Decide, Filed).
import { agentFiled, base } from "./backlog";
import type { Decision, Item, LaunchOffer, LowDisk, QueueItem } from "../proto";

export type AttnKind = "disk" | "ask" | "stuck" | "done" | "decide" | "filed";

/** The order kinds are drawn in; oldest first within each. A disk running low
 * goes first: with it full, nothing else gets done (#300). */
export const ATTN_ORDER: AttnKind[] = ["disk", "ask", "stuck", "done", "decide", "filed"];

/** What a Decide row answers: a decision proposal, or a ready item the
 * supervisor offers to start. */
export type Decide =
  | { what: "decision"; decision: Decision }
  | { what: "launch"; offer: LaunchOffer };

export type Attn =
  | { kind: "ask" | "stuck" | "done"; key: string; since: number; item: QueueItem }
  | ({ kind: "decide"; key: string; since: number } & Decide)
  /** Agent-filed backlog items in one repository, as one row. */
  | { kind: "filed"; key: string; since: number; project: string | null; items: Item[] }
  /** Every disk running low on space, as one row (#300). */
  | { kind: "disk"; key: string; since: number; low: LowDisk[] };

/** Which kind a waiting session is: a prompt is an Ask even on a flagged PR, since
 * answering it is what moves it; a flag, a block or a stale or interrupted turn
 * is Stuck; anything else finished and wants reading. */
export function sessionKind(q: QueueItem): "ask" | "stuck" | "done" {
  const s = q.status.state;
  if (s === "needs_input") return "ask";
  if (q.flag || s === "blocked" || s === "stale" || s === "interrupted") return "stuck";
  return "done";
}

/** Backlog items an agent filed that nobody has kept or dropped yet. One an agent
 * filed and took itself is its own work, already under way. */
export const filedByAgent = (item: Item) => item.status === "proposed" && !agentFiled(item);

export interface Waiting {
  queue: QueueItem[];
  proposals: Decision[];
  launches: LaunchOffer[];
  items: Item[];
  /** The disks running low, and since when (ms), as this app first saw it. */
  disk?: { low: LowDisk[]; since: number };
}

/** Everything waiting on the user, in the order it's drawn. Times in ms. */
export function attention(w: Waiting): Attn[] {
  const out: Attn[] = w.queue.map((item) => ({
    kind: sessionKind(item),
    key: `s${item.session}`,
    since: item.status.since_ms,
    item,
  }));
  for (const decision of w.proposals)
    out.push({ kind: "decide", key: `d${decision.project}#${decision.id}`, since: decision.created * 1000, what: "decision", decision });
  for (const offer of w.launches)
    out.push({ kind: "decide", key: `l${offer.project}:${offer.id}`, since: offer.made * 1000, what: "launch", offer });
  const byRepo = new Map<string, Item[]>();
  for (const item of w.items.filter(filedByAgent)) {
    const repo = item.project ?? "";
    byRepo.set(repo, [...(byRepo.get(repo) ?? []), item]);
  }
  for (const [repo, items] of byRepo)
    out.push({
      kind: "filed",
      key: `f${repo}`,
      since: Math.min(...items.map((i) => i.created)) * 1000,
      project: repo || null,
      items,
    });
  if (w.disk && w.disk.low.length > 0)
    out.push({ kind: "disk", key: "disk", since: w.disk.since, low: w.disk.low });
  const rank = (a: Attn) => ATTN_ORDER.indexOf(a.kind);
  // Stable: equal times keep the order the daemon sent them in.
  return out.sort((a, b) => rank(a) - rank(b) || a.since - b.since);
}

/** The waiting sessions' ids, in the queue's order. */
export const waitingSessions = (list: Attn[]) =>
  list.flatMap((a) => (a.kind === "decide" || a.kind === "filed" || a.kind === "disk" ? [] : [a.item.session]));

/** A Filed row's line: `3 items filed · api`. */
export function filedLine(a: { project: string | null; items: Item[] }): string {
  const n = a.items.length;
  return `${n} item${n === 1 ? "" : "s"} filed · ${a.project ? base(a.project) : "no repository"}`;
}

/** `bytes` for a person: `12.3 GB`, `512 MB`, `1.8 TB`, as the daemon says it. */
export function bytes(n: number): string {
  if (n >= 1e12) return `${(n / 1e12).toFixed(1)} TB`;
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  return `${(n / 1e6).toFixed(0)} MB`;
}

/** A Disk row's line: `8.2 GB free · ~/repos`, each disk. */
export function diskLine(low: LowDisk[]): string {
  return low.map((d) => `${bytes(d.free)} free · ${d.path}`).join(" · ");
}
