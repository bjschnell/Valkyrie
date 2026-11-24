// Where each backlog item stands (DESIGN §8.12), worked out from its sessions
// rather than dragged between columns, so it can't fall out of step.

import { ITEM_KINDS } from "../proto";
import type { Item, ItemChange, ItemFacts, ItemKind, ItemNote, KeptItem, LaunchOffer, Priority, SessionInfo, ShippedItem } from "../proto";

export type Lane = "proposed" | "needs" | "running" | "review" | "backlog" | "done";

export type Group = "progress" | "review" | "next" | "done";

/** The board's lanes, left to right as work flows (#266): Up next, In progress,
 * In review, Done. One that needs you is at the top of In progress, so a stuck
 * agent is the first thing in the lane you watch. What an agent filed and nobody
 * has kept yet isn't in a lane: it waits in the tray above them. */
export const GROUPS: { group: Group; title: string; short: string; lanes: Lane[] }[] = [
  { group: "next", title: "Up next", short: "Next", lanes: ["backlog"] },
  { group: "progress", title: "In progress", short: "Doing", lanes: ["needs", "running"] },
  { group: "review", title: "In review", short: "Review", lanes: ["review"] },
  { group: "done", title: "Done", short: "Done", lanes: ["done"] },
];

/** Where j, k, h or l moves the board's cursor, `[lane, row]`, given each lane's
 * card count. j and k go down and up a lane and roll into the next or previous
 * lane with cards at its end; h and l jump lanes, keeping the row where they can.
 * A cursor left past a lane that shrank lands on its last card first. */
export function boardStep(sizes: number[], at: [number, number] | null, key: "j" | "k" | "h" | "l"): [number, number] | null {
  const filled = sizes.flatMap((n, i) => (n > 0 ? [i] : []));
  if (filled.length === 0) return null;
  if (!at) return [filled[0], 0];
  const [l, r] = at;
  if (!sizes[l]) return [filled.find((i) => i > l) ?? filled[filled.length - 1], 0];
  if (r >= sizes[l]) return [l, sizes[l] - 1];
  const next = filled.find((i) => i > l);
  const prev = [...filled].reverse().find((i) => i < l);
  switch (key) {
    case "j":
      return r + 1 < sizes[l] ? [l, r + 1] : next !== undefined ? [next, 0] : [l, r];
    case "k":
      return r > 0 ? [l, r - 1] : prev !== undefined ? [prev, sizes[prev] - 1] : [l, r];
    case "l":
      return next !== undefined ? [next, Math.min(r, sizes[next] - 1)] : [l, r];
    case "h":
      return prev !== undefined ? [prev, Math.min(r, sizes[prev] - 1)] : [l, r];
  }
}

/** The lane a phone opens on, showing one at a time: In progress when anything
 * is, else the first lane with cards, else Up next. */
export function firstLane(groups: Record<Group, unknown[]>): Group {
  if (groups.progress.length) return "progress";
  return GROUPS.find(({ group }) => groups[group].length)?.group ?? "next";
}

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

/** The items that are pieces of `item`, the oldest first. */
export const pieces = (item: Item, items: Item[]): Item[] => items.filter((i) => i.parent === item.id);

/** How many of `item`'s pieces are shipped or dropped, of how many it has. */
export function progress(item: Item, items: Item[]): [number, number] {
  const all = pieces(item, items);
  return [all.filter((i) => i.status === "shipped" || i.status === "dropped").length, all.length];
}

/** What people call item `id`: its number in its repository, or `id` for one that's gone. */
export const itemNo = (items: Item[], id: number): number => items.find((i) => i.id === id)?.number ?? id;

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

/** Items by group, each with the lane it's in, and the tray's: what agents filed
 * that nobody has kept or dropped. */
export function byGroup(
  items: Item[],
  sessions: SessionInfo[],
  boot: number,
  all: Item[] = items,
): { filed: Item[]; groups: Record<Group, { item: Item; lane: Lane }[]> } {
  const lanes = byLane(items, sessions, boot, all);
  const groups = {} as Record<Group, { item: Item; lane: Lane }[]>;
  for (const { group, lanes: of } of GROUPS) groups[group] = of.flatMap((lane) => lanes[lane].map((item) => ({ item, lane })));
  return { filed: lanes.proposed, groups };
}

/** A group's rows as a tree: each piece right under the item it's a piece of, one
 * step in, when both are in the group; otherwise where its order puts it. */
export function tree<T extends { item: Item }>(rows: T[]): (T & { depth: number })[] {
  const here = new Set(rows.map((r) => r.item.id));
  const out: (T & { depth: number })[] = [];
  const visit = (r: T, depth: number) => {
    out.push({ ...r, depth });
    for (const c of rows) if (c.item.parent === r.item.id && c.item.id !== r.item.id) visit(c, depth + 1);
  };
  for (const r of rows) if (r.item.parent == null || !here.has(r.item.parent)) visit(r, 0);
  return out;
}

/** What an item is called in a list of several repositories: `api#2`, or `#2`. */
export const itemRef = (item: Item) => `${item.project ? base(item.project) : ""}#${item.number}`;

/** A row's state as a dot and a word (#252): the dot is a session state's color.
 * `flag` is the supervisor's reason on its live session (`CI red on PR #41: …`). */
export function rowState(
  item: Item,
  lane: Lane,
  items: Item[],
  flag?: string,
): { state: "needs_input" | "blocked" | "working" | "review_ready" | "idle" | "exited"; word: string } {
  if (flag) return { state: "blocked", word: flag.split(":")[0] };
  switch (lane) {
    case "needs":
      return { state: "needs_input", word: "needs you" };
    case "running":
      return { state: "working", word: agentFiled(item) ? "running · not accepted" : "running" };
    case "review":
      if (item.pr_closed) return { state: "idle", word: "PR closed" };
      return { state: "review_ready", word: item.pr ? `PR #${item.pr.split("/").pop()} open` : "to review" };
    case "done":
      return { state: "exited", word: item.status };
    default: {
      const waits = waitingOn(item, items);
      return { state: "idle", word: waits.length ? `after ${waits.map((id) => `#${itemNo(items, id)}`).join(", ")}` : "ready" };
    }
  }
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

/** Which kind of item the board shows: every kind (`null`) or one. */
export type KindFilter = ItemKind | null;

export const ofKind = (item: Item, kind: KindFilter) => kind === null || item.kind === kind;

/** A kind filter as kept between visits; anything else is every kind. */
export const kindFilter = (saved: string | null): KindFilter => ITEM_KINDS.find((k) => k === saved) ?? null;

/** How many of `items` are still to do (open or proposed), of each kind, to show
 * beside its chip. */
export function kindCounts(items: Item[]): Record<ItemKind, number> {
  const out = Object.fromEntries(ITEM_KINDS.map((k) => [k, 0])) as Record<ItemKind, number>;
  for (const i of items) if (i.status === "open" || i.status === "proposed") out[i.kind]++;
  return out;
}

/** Where an agent's note came from when that kept it out of the brief, as the
 * CLI and TUI say it (`ItemNote::origin`): ` · while on #9`, or empty. */
export function noteOrigin(n: ItemNote): string {
  if (n.aside) return n.from ? ` · while on #${n.from_number || n.from}` : " · from outside the item";
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
  const parts = [shipped.length ? `Shipped ${shipped.map((s) => `#${s.number || s.id}`).join(" ")}` : "Nothing merged to ship"];
  const notMerged = kept.filter((k) => k.why === "not merged").length;
  if (notMerged) parts.push(`${notMerged} not merged`);
  for (const k of kept) if (k.why !== "not merged") parts.push(`#${k.number || k.id} ${k.why}`);
  return `${parts.join(" · ")} (as of the last fetch)`;
}

/** What a first tap found, to check before the second ships it:
 * `Tap again to ship #3 fix/old (set by a take), merged into origin/main · …`. */
export function wouldShip(shipped: ShippedItem[], kept: KeptItem[]): string {
  const mark = (s: ShippedItem) =>
    s.filed ? " (agent-filed, not yet accepted)" : s.branch_taken ? " (set by a take)" : "";
  const found = shipped.map((s) => `#${s.number || s.id} ${s.branch}${mark(s)}, merged into ${s.against}`);
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
  mine: { title: string; body: string; kind: ItemKind; project: string | null; priority?: Priority; blocked_by?: number[]; parent?: number },
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
  // 0 for none, as the daemon takes it.
  if (mine.parent !== undefined && other("group", mine.parent, was.parent ?? 0, (now ?? was).parent ?? 0)) {
    change.parent = mine.parent;
    if (now) seen = { ...seen, parent: now.parent };
  }
  if (clash.length) {
    const what = clash.join(" and ");
    return { clash: `#${was.number}'s ${what} changed elsewhere since you opened it: save again to replace it`, was: seen };
  }
  return Object.keys(change).length > 1 ? change : null;
}

/** The launch offers the board shows: the shown repository's, or every one. */
export const launchesShown = (offers: LaunchOffer[], repo: RepoFilter) =>
  offers.filter((o) => repo === null || o.project === repo);

/** What a launch offer starts: `claude`, `codex · gpt-5`. */
export const launchesOn = (o: LaunchOffer) => (o.model ? `${o.agent} · ${o.model}` : o.agent);

/** The agent `item` starts on: the one it was last launched with (decision #46),
 * else `fallback` (the agent last picked on this device), else claude. */
export function startAgent(item: Item | null | undefined, fallback: string | null): "claude" | "codex" {
  const last = item?.launch?.agent;
  if (last === "claude" || last === "codex") return last;
  return fallback === "codex" ? "codex" : "claude";
}

/** Claude Code's own aliases, which follow its newest models; Codex's names change
 * too often to list (the TUI's launch form has the same split). */
export const MODEL_CHOICES: Record<"claude" | "codex", string[]> = {
  claude: ["opus", "sonnet", "haiku"],
  codex: [],
};

/** The model the Launch field starts on: what `item` last ran with `agent`, else
 * empty (the daemon then keeps whatever the item's last launch of it ran). */
export function startModel(item: Item | null | undefined, agent: string): string {
  return item?.launch?.agent === agent ? (item.launch.model ?? "") : "";
}

/** The `model` for `launch_item`: nothing typed is `undefined` (as last time), and
 * `default` is `""`, the agent's own, as `valk todo launch --model default` has it. */
export function launchModel(typed: string): string | undefined {
  const m = typed.trim();
  if (!m) return undefined;
  return m.toLowerCase() === "default" ? "" : m;
}
