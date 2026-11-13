// Where each backlog item stands (DESIGN §8.12), worked out from its sessions
// rather than dragged between columns, so it can't fall out of step.

import type { Item, ItemChange, ItemFacts, ItemKind, ItemNote, KeptItem, LaunchOffer, OrderApplied, OrderProposal, OrderStep, Priority, SessionInfo, ShippedItem } from "../proto";

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
export const agentFiled = (item: Item) => item.status === "proposed" && !!item.filed_by && !!item.taken?.length;

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
      case "interrupted":
        // An Esc-interrupted turn finished nothing: it isn't up for review.
        return item.pr ? "review" : "running";
      default:
        // Done with its turn or idle: its work is waiting on you.
        return "review";
    }
  }
  return item.pr || (item.sessions?.length ?? 0) > 0 ? "review" : "backlog";
}

export const priority = (item: Item): Priority => item.priority ?? "p2";

/** Those of what `item` comes after that aren't shipped or dropped yet; a deleted one
 * is gone, so it waits on nothing. */
export function waitingOn(item: Item, items: Item[]): number[] {
  return (item.blocked_by ?? []).filter((id) =>
    items.some((i) => i.id === id && (i.status === "open" || i.status === "proposed")),
  );
}

/** The order to work items in, as the daemon's `order`: those waiting on nothing,
 * then by priority, then the oldest. */
export function order(items: Item[]): (a: Item, b: Item) => number {
  const waits = (i: Item) => (waitingOn(i, items).length ? 1 : 0);
  return (a, b) => waits(a) - waits(b) || priority(a).localeCompare(priority(b)) || a.id - b.id;
}

/** Items by lane, each in the order to work it (`order`); the done ones newest
 * first, by when they were closed. `all` is every item, for what they wait on. */
export function byLane(items: Item[], sessions: SessionInfo[], boot: number, all: Item[] = items): Record<Lane, Item[]> {
  const out: Record<Lane, Item[]> = { proposed: [], needs: [], running: [], review: [], backlog: [], done: [] };
  for (const item of items) out[lane(item, sessions, boot)].push(item);
  const work = order(all);
  for (const [l, list] of Object.entries(out)) {
    list.sort(l === "done" ? (a, b) => b.updated - a.updated || b.id - a.id : work);
  }
  return out;
}

/** Items by column, each with the lane it's in. */
export function byColumn(
  items: Item[],
  sessions: SessionInfo[],
  boot: number,
  all: Item[] = items,
): Record<Column, { item: Item; lane: Lane }[]> {
  const lanes = byLane(items, sessions, boot, all);
  const out = {} as Record<Column, { item: Item; lane: Lane }[]>;
  for (const { column, lanes: of } of COLUMNS) out[column] = of.flatMap((lane) => lanes[lane].map((item) => ({ item, lane })));
  return out;
}

/** The last part of a path: a repository's name. */
export const base = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** Which items the board shows: every repository's (`null`), one repository's (its
 * path), or those with no folder yet (`""`). */
export type RepoFilter = string | null;

export const inRepo = (item: Item, repo: RepoFilter) => repo === null || (item.project ?? "") === repo;

/** The repositories items are for, to pick from: each with its name, the full path
 * where two share one; then "No folder" if any item has none. Sorted by name. */
export function repoChoices(items: Item[]): { repo: string; label: string }[] {
  const paths = [...new Set(items.flatMap((i) => (i.project ? [i.project] : [])))];
  const named = paths.map((repo) => ({ repo, label: base(repo) }));
  const clash = (label: string) => named.filter((n) => n.label === label).length > 1;
  const out = named.map((n) => (clash(n.label) ? { ...n, label: n.repo } : n));
  out.sort((a, b) => a.label.localeCompare(b.label) || a.repo.localeCompare(b.repo));
  if (items.some((i) => !i.project)) out.push({ repo: "", label: "No folder" });
  return out;
}

/** Where an agent's note came from when that kept it out of the brief, as the
 * CLI and TUI say it (`ItemNote::origin`): ` · while on #9`, or empty. */
export function noteOrigin(n: ItemNote): string {
  if (n.aside) return n.from ? ` · while on #${n.from}` : " · from outside the item";
  if (n.taken) return " · from a take";
  return "";
}

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
  mine: { title: string; body: string; kind: ItemKind; project: string | null; priority?: Priority; blocked_by?: number[] },
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
  // Compared as the daemon keeps them: no priority is p2, the list sorted.
  const ids = (l: number[] | undefined) => [...new Set(l ?? [])].sort((a, b) => a - b).join(",");
  const other = <V>(what: string, mineV: V, wasV: V, nowV: V) => {
    if (mineV === wasV) return false;
    if (now && nowV !== wasV && nowV !== mineV) clash.push(what);
    return true;
  };
  if (mine.priority && other("priority", mine.priority, priority(was), now ? priority(now) : priority(was))) {
    change.priority = mine.priority;
    if (now) seen = { ...seen, priority: now.priority };
  }
  if (mine.blocked_by && other("order", ids(mine.blocked_by), ids(was.blocked_by), ids((now ?? was).blocked_by))) {
    change.blocked_by = mine.blocked_by;
    if (now) seen = { ...seen, blocked_by: now.blocked_by };
  }
  if (clash.length) {
    const what = clash.join(" and ");
    return { clash: `#${was.id}'s ${what} changed elsewhere since you opened it: save again to replace it`, was: seen };
  }
  return Object.keys(change).length > 1 ? change : null;
}

/** The repository a build order is asked for from the board: the one it shows, else
 * the only one its open items are in; null when that's no one or several. */
export function planScope(repo: RepoFilter, items: Item[]): string | null {
  if (repo !== null) return repo || null;
  const roots = new Set(items.filter((i) => i.status === "open" && i.project).map((i) => i.project as string));
  return roots.size === 1 ? [...roots][0] : null;
}

/** The launch offers the board shows: the shown repository's, or every one. */
export const launchesShown = (offers: LaunchOffer[], repo: RepoFilter) =>
  offers.filter((o) => repo === null || o.project === repo);

/** What a launch offer starts: `claude`, `codex · gpt-5`. */
export const launchesOn = (o: LaunchOffer) => (o.model ? `${o.agent} · ${o.model}` : o.agent);

/** The build orders the board shows: the shown repository's, or every one. */
export const ordersShown = (orders: OrderProposal[], repo: RepoFilter) =>
  orders.filter((o) => repo === null || o.project === repo);

/** What accepting `step` changes on `item`: `p2→p1, after #37`; empty for nothing. */
export function stepChanges(step: OrderStep, item: Item | undefined): string {
  const parts: string[] = [];
  if (step.priority) parts.push(item ? `${priority(item)}→${step.priority}` : step.priority);
  if (step.blocked_by) {
    const had = item?.blocked_by ?? [];
    const added = step.blocked_by.filter((b) => !had.includes(b));
    if (added.length) parts.push(`after ${added.map((b) => `#${b}`).join(", ")}`);
  }
  return parts.join(", ");
}

/** Whether accepting `step` changes anything. */
export const changesSomething = (step: OrderStep) => step.priority !== undefined || step.blocked_by !== undefined;

/** The toast after answering a build order. */
export function appliedSummary(accept: boolean, applied: OrderApplied): string {
  if (!accept) return "Build order declined: nothing changed";
  const changed = applied.changed.length ? `Changed ${applied.changed.map((i) => `#${i}`).join(" ")}` : "Nothing changed";
  const left = (applied.skipped ?? []).map(([id, why]) => `#${id} left: ${why}`);
  return [changed, ...left].join("; ");
}

/** The agent `item` starts on: the one it was last launched with (decision #46),
 * else `fallback` (the agent last picked on this device), else claude. */
export function startAgent(item: Item | null | undefined, fallback: string | null): "claude" | "codex" {
  const last = item?.launch?.agent;
  if (last === "claude" || last === "codex") return last;
  return fallback === "codex" ? "codex" : "claude";
}
