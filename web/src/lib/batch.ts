// Start agents (backlog #282): several tasks at once, one session each. The
// composer's lines, or backlog items picked beside them, each with the repository
// and agent it starts on, to check and change before anything runs. The pure parts
// live here; the sheet is components/StartAgents.tsx.

import type { Item, SessionInfo } from "../proto";
import { base, itemRef, liveSession, order, startAgent } from "./backlog";
import { foundItem, guessRepo, lastRan, type Found, type Ran } from "./compose";

/** One session to start: a typed task, or a backlog item (its repo fixed). */
export interface Row {
  /** Stable across edits, for React and the cursor. */
  key: number;
  task: string;
  /** The item's global id when the row is a backlog item (decision #73). */
  item: number | null;
  repo: string | null;
  ran: Ran;
  /** Why it can't start: blocked, or already has a session. */
  why: string | null;
  /** Why it didn't start, the last time Start tried. */
  failed: string | null;
}

/** What the sheet guesses with: the repos offered, the last one used, what each last ran. */
export interface Context {
  repos: string[];
  last: string | null;
  remembered: Record<string, Ran>;
  items: Item[];
  sessions: SessionInfo[];
  boot: number;
}

let next = 1;

/** A backlog item as a row: its repo, and the agent it last ran (decision #46). */
export function itemRow(f: Found, c: Context): Row {
  const repo = f.item.project ?? null;
  const ran = lastRan(repo, c.remembered, c.items, c.sessions);
  const agent = startAgent(f.item, ran.agent);
  const model = f.item.launch?.agent === agent ? f.item.launch.model : agent === ran.agent ? ran.model : undefined;
  return {
    key: next++,
    task: f.item.title || f.item.body.split("\n")[0],
    item: f.item.id,
    repo,
    ran: model ? { agent, model } : { agent },
    why: f.pick === "open" ? "has a session" : f.why,
    failed: null,
  };
}

/** A typed task as a row: the repo it names, else none, and what that repo last ran. */
export function taskRow(task: string, c: Context): Row {
  const repo = guessRepo(task, c.repos);
  return { key: next++, task, item: null, repo, ran: lastRan(repo, c.remembered, c.items, c.sessions), why: null, failed: null };
}

/**
 * The open item a line names by number (`api#7`, `#7`), if exactly one does: a
 * bare number prefers the last repo used.
 */
export function namedItem(line: string, items: Item[], last: string | null): Item | null {
  const m = /^([\w.-]+)?#(\d+)$/.exec(line.trim().toLowerCase());
  if (!m) return null;
  const number = Number(m[2]);
  const hits = items.filter((i) => i.status === "open" && i.number === number && (!m[1] || base(i.project ?? "").toLowerCase() === m[1]));
  if (hits.length === 1) return hits[0];
  return hits.find((i) => i.project === last) ?? null;
}

/** A row per non-empty line: an item where the line names one, else a task. */
export function rowsFrom(text: string, c: Context): Row[] {
  return lines(text).map((line) => {
    const item = namedItem(line, c.items, c.last);
    return item ? itemRow(foundItem(item, c.items, c.sessions, c.boot), c) : taskRow(line, c);
  });
}

/** The non-empty lines of `text`, trimmed. */
export const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

/**
 * Dictated phrases as tasks: each pause is a new line, and a phrase that opens
 * with "next" starts one too, without the word.
 */
export function spoken(phrases: string): string[] {
  return lines(phrases)
    .flatMap((p) => p.split(/(?:^|[.,;]\s*)next\b[.,:]?\s*/i))
    .map((p) => p.trim().replace(/[.,;]$/, ""))
    .filter(Boolean)
    .map((p) => p[0].toUpperCase() + p.slice(1));
}

/**
 * The backlog items to pick from: open, in a repository, nobody on them yet and
 * no PR, in the order to work them; ready ones first, blocked ones after them
 * (greyed, decision #77). `skip`: launch offers already in Needs you.
 */
export function pickable(items: Item[], sessions: SessionInfo[], boot: number, skip: Set<number>, n = 6): Found[] {
  const found = items
    .filter((i) => i.status === "open" && !!i.project && !skip.has(i.id) && !i.pr && (i.sessions?.length ?? 0) === 0 && !liveSession(i, sessions, boot))
    .sort(order(items))
    .map((i) => foundItem(i, items, sessions, boot));
  const ready = found.filter((f) => f.pick === "start");
  return [...ready, ...found.filter((f) => f.pick !== "start")].slice(0, Math.max(n, 0));
}

/** `3 agents · 2 repos`: how many start, and where. */
export function summary(rows: Row[]): string {
  const n = rows.length;
  const repos = new Set(rows.map((r) => r.repo ?? "")).size;
  return `${n} agent${n === 1 ? "" : "s"} · ${repos} repo${repos === 1 ? "" : "s"}`;
}

/** The draft the rows give back to the composer: typed tasks as they are, items by their number. */
export function draft(rows: Row[], items: Item[]): string {
  return rows
    .map((r) => {
      const item = r.item === null ? undefined : items.find((i) => i.id === r.item);
      return item ? itemRef(item) : r.task;
    })
    .join("\n");
}
