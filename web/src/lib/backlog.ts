// Where each backlog item stands (DESIGN §8.12), worked out from its sessions
// rather than dragged between columns, so it can't fall out of step.

import type { Item, ItemChange, ItemFacts, ItemKind, KeptItem, SessionInfo, ShippedItem } from "../proto";

export type Lane = "proposed" | "needs" | "running" | "review" | "backlog" | "done";

export type Column = "backlog" | "progress" | "review" | "done";

/** The board's columns, left to right. One that needs you is in progress, at its
 * top, so a stuck agent is the first thing in the column you watch; one an agent
 * proposed is at the top of the backlog, waiting on you to accept it. */
export const COLUMNS: { column: Column; title: string; lanes: Lane[] }[] = [
  { column: "backlog", title: "Backlog", lanes: ["proposed", "backlog"] },
  { column: "progress", title: "In progress", lanes: ["needs", "running"] },
  { column: "review", title: "Review", lanes: ["review"] },
  { column: "done", title: "Done", lanes: ["done"] },
];

/** The newest of an item's sessions still running on this daemon, if any. */
export function liveSession(item: Item, sessions: SessionInfo[], boot: number): SessionInfo | null {
  if (item.boot !== boot) return null;
  for (const id of [...(item.sessions ?? [])].reverse()) {
    const s = sessions.find((s) => s.id === id && s.exited === null);
    if (s) return s;
  }
  return null;
}

/** An open item whose live sessions only took it: the user's launch hands it over, as
 * `valk todo launch` does, so the card offers Launch beside Open. A take never locks
 * the user out. */
export function takenOnly(item: Item, sessions: SessionInfo[], boot: number): boolean {
  if (item.status !== "open" || item.boot !== boot) return false;
  const live = (item.sessions ?? []).filter((id) => sessions.some((s) => s.id === id && s.exited === null));
  return live.length > 0 && live.every((id) => (item.taken ?? []).includes(id));
}

/** Written down and taken by an agent for its own work, not accepted yet: it
 * follows its session like an open item, and shipping it accepts it. */
export const agentFiled = (item: Item) => item.status === "proposed" && !!item.filed_by;

export type CardAction = "accept" | "open" | "launch" | "folder";

/** What an open card offers, in order, the last one primary: Accept for an agent's own
 * filed work (even while its session runs), Open for a live session, and Launch (or
 * Pick a folder) when nothing holds it or only takes do. */
export function cardActions(item: Item, sessions: SessionInfo[], boot: number): CardAction[] {
  const live = liveSession(item, sessions, boot);
  const out: CardAction[] = [];
  if (agentFiled(item)) out.push("accept");
  if (live) out.push("open");
  if (agentFiled(item) || (live && !takenOnly(item, sessions, boot))) return out;
  out.push(item.project ? "launch" : "folder");
  return out;
}

/** Where an item stands. One with a pull request open waits on review whatever its
 * session does, unless that needs you: a session that took several items would
 * otherwise show them all running. */
export function lane(item: Item, sessions: SessionInfo[], boot: number): Lane {
  if (item.status === "proposed" && !agentFiled(item)) return "proposed";
  if (item.status !== "open" && !agentFiled(item)) return "done";
  const live = liveSession(item, sessions, boot);
  if (live) {
    switch (live.status.state) {
      case "needs_input":
      case "blocked":
        return "needs";
      case "working":
      case "stale":
        return item.pr ? "review" : "running";
      default:
        // Done with its turn, idle or interrupted: its work is waiting on you.
        return "review";
    }
  }
  return item.pr || item.checkout || (item.sessions?.length ?? 0) > 0 ? "review" : "backlog";
}

/** Items by lane, each lane newest first; the done ones by when they were closed. */
export function byLane(items: Item[], sessions: SessionInfo[], boot: number): Record<Lane, Item[]> {
  const out: Record<Lane, Item[]> = { proposed: [], needs: [], running: [], review: [], backlog: [], done: [] };
  for (const item of items) out[lane(item, sessions, boot)].push(item);
  for (const l of Object.values(out)) l.sort((a, b) => b.updated - a.updated || b.id - a.id);
  return out;
}

/** Items by column, each with the lane it's in. */
export function byColumn(
  items: Item[],
  sessions: SessionInfo[],
  boot: number,
): Record<Column, { item: Item; lane: Lane }[]> {
  const lanes = byLane(items, sessions, boot);
  const out = {} as Record<Column, { item: Item; lane: Lane }[]>;
  for (const { column, lanes: of } of COLUMNS) out[column] = of.flatMap((lane) => lanes[lane].map((item) => ({ item, lane })));
  return out;
}

/** The last part of a path: a repository's name. */
export const base = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** `2 commits · 3 files changed, 40 insertions(+) · 1 not committed`. */
export function factsLine(f: ItemFacts): string {
  const parts = [f.ahead === 1 ? "1 commit" : `${f.ahead} commits`];
  if (f.diff) parts.push(f.diff);
  if (f.dirty > 0) parts.push(`${f.dirty} not committed`);
  return parts.join(" · ");
}

/** The pull request to link to: only an `https://` one, never `javascript:` and the like. */
export function prLink(pr: string | undefined): string | null {
  return pr && /^https:\/\/\S+$/.test(pr) ? pr : null;
}

/** `Shipped #1 #4 · 2 not merged · #9 branch fix/x not found (as of the last fetch)`. */
export function shippedSummary(shipped: ShippedItem[], kept: KeptItem[]): string {
  const parts = [shipped.length ? `Shipped ${shipped.map((s) => `#${s.id}`).join(" ")}` : "Nothing merged to ship"];
  const notMerged = kept.filter((k) => k.why === "not merged").length;
  if (notMerged) parts.push(`${notMerged} not merged`);
  for (const k of kept) if (k.why !== "not merged") parts.push(`#${k.id} ${k.why}`);
  return `${parts.join(" · ")} (as of the last fetch)`;
}

/** What a first tap found, to check before the second ships it:
 * `Tap again to ship #3 fix/old (set by a take), merged into origin/main · …`. */
export function wouldShip(shipped: ShippedItem[], kept: KeptItem[]): string {
  const mark = (s: ShippedItem) =>
    s.filed ? " (agent-filed, not yet accepted)" : s.branch_taken ? " (set by a take)" : "";
  const found = shipped.map((s) => `#${s.id} ${s.branch}${mark(s)}, merged into ${s.against}`);
  const notMerged = kept.filter((k) => k.why === "not merged").length;
  const rest = notMerged ? ` · ${notMerged} not merged` : "";
  return `Tap again to ship ${found.join(" · ")}${rest} (as of the last fetch)`;
}

/**
 * The edit to send: only the fields changed in the editor (`null` if none), so a
 * change made elsewhere since it opened stays. A field changed both here and
 * there is refused, as a message, rather than one quietly winning, with `was`
 * moved on for those fields only: saving again then replaces them, and still
 * leaves alone every other field changed elsewhere. Text is compared trimmed,
 * as the daemon stores it.
 */
export function editChange(
  was: Item | null,
  now: Item | null,
  mine: { title: string; body: string; kind: ItemKind; project: string | null },
): ItemChange | { clash: string; was: Item } | null {
  if (!was) return null;
  const change: Extract<ItemChange, { do: "edit" }> = { do: "edit" };
  const clash: string[] = [];
  let seen = was;
  const field = <K extends "title" | "body" | "kind" | "project">(k: K, value: Item[K]) => {
    const norm = (v: Item[K] | undefined) => (typeof v === "string" ? v.trim() : v);
    const [mineV, wasV] = [norm(value), norm(was[k])];
    if (mineV === wasV) return false;
    const nowV = now ? norm(now[k]) : wasV;
    if (now && nowV !== wasV && nowV !== mineV) {
      clash.push(k === "project" ? "repository" : k);
      seen = { ...seen, [k]: now[k] };
    }
    return true;
  };
  if (field("title", mine.title)) change.title = mine.title;
  if (field("body", mine.body)) change.body = mine.body;
  if (field("kind", mine.kind)) change.kind = mine.kind;
  if (mine.project !== null && field("project", mine.project)) change.cwd = mine.project;
  if (clash.length) {
    const what = clash.join(" and ");
    return { clash: `#${was.id}'s ${what} changed elsewhere since you opened it: save again to replace it`, was: seen };
  }
  return Object.keys(change).length > 1 ? change : null;
}
