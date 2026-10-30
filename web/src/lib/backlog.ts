// Where each backlog item stands (DESIGN §8.12), worked out from its sessions
// rather than dragged between columns, so it can't fall out of step.

import type { Item, ItemChange, ItemFacts, ItemKind, SessionInfo } from "../proto";

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

export function lane(item: Item, sessions: SessionInfo[], boot: number): Lane {
  if (item.status === "proposed") return "proposed";
  if (item.status !== "open") return "done";
  const live = liveSession(item, sessions, boot);
  if (live) {
    switch (live.status.state) {
      case "needs_input":
      case "blocked":
        return "needs";
      case "working":
      case "stale":
        return "running";
      default:
        // Done with its turn, idle or interrupted: its work is waiting on you.
        return "review";
    }
  }
  return item.checkout || (item.sessions?.length ?? 0) > 0 ? "review" : "backlog";
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

/**
 * The edit to send: only the fields changed in the editor (`null` if none), so a
 * change made elsewhere since it opened stays. A field changed both here and
 * there is refused, as a message, rather than one quietly winning.
 */
export function editChange(
  was: Item | null,
  now: Item | null,
  mine: { title: string; body: string; kind: ItemKind; project: string | null },
): ItemChange | string | null {
  if (!was) return null;
  const change: Extract<ItemChange, { do: "edit" }> = { do: "edit" };
  const clash: string[] = [];
  const field = <K extends "title" | "body" | "kind" | "project">(k: K, value: Item[K]) => {
    if (value === was[k]) return false;
    if (now && now[k] !== was[k] && now[k] !== value) clash.push(k === "project" ? "repository" : k);
    return true;
  };
  if (field("title", mine.title)) change.title = mine.title;
  if (field("body", mine.body)) change.body = mine.body;
  if (field("kind", mine.kind)) change.kind = mine.kind;
  if (mine.project !== null && field("project", mine.project)) change.cwd = mine.project;
  if (clash.length)
    return `#${was.id}'s ${clash.join(" and ")} changed elsewhere since you opened it: go back and edit it again`;
  return Object.keys(change).length > 1 ? change : null;
}
