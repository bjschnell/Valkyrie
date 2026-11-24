// The home composer (backlog #275): what typing into it does. A line starts a
// session, `#` finds a backlog item, several lines go to the launcher. The pure
// parts live here so they can be tested without a page.

import type { Item, SessionInfo } from "../proto";
import { base, itemRef, liveSession, order, waitingOn } from "./backlog";

export type ComposeAgent = "claude" | "codex";

/** An agent and the model it runs; `model` absent is the agent's own default. */
export interface Ran {
  agent: ComposeAgent;
  model?: string;
}

/** What the box holds: nothing, a task, a `#` search, or several lines. */
export type Intent =
  | { kind: "empty" }
  | { kind: "task"; text: string }
  | { kind: "find"; query: string }
  | { kind: "batch"; lines: string[] };

export function intent(text: string): Intent {
  const lines = text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
  if (lines.length === 0) return { kind: "empty" };
  if (lines.length > 1) return { kind: "batch", lines };
  const line = lines[0];
  if (line.startsWith("#")) return { kind: "find", query: line.slice(1).trim() };
  return { kind: "task", text: line };
}

/**
 * The repository a task is about: one whose name the text says as a word (the
 * longest, when several do, so `valkyrie-web` beats `valkyrie`), else none: a
 * task that names no repo isn't put in one (backlog #292).
 */
export function guessRepo(text: string, repos: string[]): string | null {
  const words = text.toLowerCase();
  let best: string | null = null;
  for (const repo of repos) {
    const name = base(repo).toLowerCase();
    if (!name) continue;
    const at = new RegExp(`(^|[^\\w-])${escape(name)}($|[^\\w-])`);
    if (at.test(words) && (best === null || name.length > base(best).length)) best = repo;
  }
  return best;
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

const isAgent = (a: string | undefined): a is ComposeAgent => a === "claude" || a === "codex";

/**
 * What `repo` last ran (decision #46): what this composer last started there,
 * else the newest item launch in it, else the agent of a session running in
 * it, else claude with its own model.
 */
export function lastRan(
  repo: string | null,
  remembered: Record<string, Ran>,
  items: Item[],
  sessions: SessionInfo[],
): Ran {
  if (!repo) return { agent: "claude" };
  const mine = remembered[repo];
  if (mine && isAgent(mine.agent)) return mine;
  const launched = items
    .filter((i) => i.project === repo && isAgent(i.launch?.agent))
    .sort((a, b) => b.updated - a.updated)[0]?.launch;
  if (launched && isAgent(launched.agent)) return launched.model ? { agent: launched.agent, model: launched.model } : { agent: launched.agent };
  const inside = (cwd: string) => cwd === repo || cwd.startsWith(repo + "/");
  const live = sessions
    .filter((s) => s.exited === null && inside(s.cwd) && isAgent(s.status.agent))
    .sort((a, b) => b.created_unix - a.created_unix)[0];
  if (live) return { agent: live.status.agent as ComposeAgent };
  return { agent: "claude" };
}

/** The agent's command line for a first message. */
export function command(ran: Ran, text: string): string[] {
  return [ran.agent, ...(ran.model ? ["--model", ran.model] : []), text];
}

/** How a chip reads: `claude`, `claude · opus`. */
export const ranLabel = (r: Ran) => (r.model ? `${r.agent} · ${r.model}` : r.agent);

/** A session's name, which its worktree's branch is named after: the first few
 * words, lower case, joined by dashes. */
export function slug(text: string): string {
  const words = text
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, " ")
    .split(/[\s-]+/)
    .filter(Boolean);
  let out = "";
  for (const w of words) {
    const next = out ? `${out}-${w}` : w;
    if (next.length > 40) break;
    out = next;
    if (out.split("-").length >= 6) break;
  }
  return out || "task";
}

/** What an item in the `#` list does when picked. */
export type Pick = "start" | "open" | "blocked";

export interface Found {
  item: Item;
  /** `start` a ready one, `open` the session already on it, or nothing: `blocked`. */
  pick: Pick;
  /** The live session to open. */
  session: SessionInfo | null;
  /** Why it can't start: `after web#1`, `no repo`. */
  why: string | null;
}

/** What picking `item` does: open its session, start it, or nothing yet (and why). */
export function foundItem(item: Item, items: Item[], sessions: SessionInfo[], boot: number): Found {
  const session = liveSession(item, sessions, boot);
  if (session) return { item, pick: "open", session, why: null };
  const waits = waitingOn(item, items);
  if (waits.length) {
    const refs = waits.map((id) => {
      const w = items.find((i) => i.id === id);
      return w ? itemRef(w) : `#${id}`;
    });
    return { item, pick: "blocked", session: null, why: `after ${refs.join(", ")}` };
  }
  if (!item.project) return { item, pick: "blocked", session: null, why: "no repo" };
  return { item, pick: "start", session: null, why: null };
}

/**
 * Open items matching a `#` search, by number (`12`, `web#12`) or by words of
 * the title, across repositories: exact numbers first, then in the order to
 * work them. At most `limit`.
 */
export function findItems(query: string, items: Item[], sessions: SessionInfo[], boot: number, limit = 6): Found[] {
  const open = items.filter((i) => i.status === "open");
  const q = query.trim().toLowerCase();
  const num = /^(?:([\w.-]+)#)?(\d+)$/.exec(q);
  const words = q.split(/\s+/).filter(Boolean);
  const exact = (i: Item) => !!num && i.number === Number(num[2]) && (!num[1] || base(i.project ?? "").toLowerCase() === num[1]);
  const hit = (i: Item) => {
    if (exact(i)) return true;
    if (num) return false;
    const title = (i.title || i.body.split("\n")[0]).toLowerCase();
    return words.every((w) => title.includes(w) || base(i.project ?? "").toLowerCase() === w);
  };
  const work = order(items);
  return open
    .filter(hit)
    .sort((a, b) => Number(exact(b)) - Number(exact(a)) || work(a, b))
    .slice(0, limit)
    .map((i) => foundItem(i, items, sessions, boot));
}

/**
 * The top `n` items ready to start: open, in a repository, nobody on them,
 * waiting on nothing, never started and with no PR, in the order to work them.
 * `skip` are items shown elsewhere on the screen already (a launch offer in
 * Needs you), so none is drawn twice (decision #77, rule 3).
 */
export function readyItems(items: Item[], sessions: SessionInfo[], boot: number, skip: Set<number>, n = 3): Item[] {
  return items
    .filter(
      (i) =>
        i.status === "open" &&
        !!i.project &&
        !skip.has(i.id) &&
        !i.pr &&
        (i.sessions?.length ?? 0) === 0 &&
        !liveSession(i, sessions, boot) &&
        waitingOn(i, items).length === 0,
    )
    .sort(order(items))
    .slice(0, n);
}

/** The supervisor's line under the composer: `supervisor watching 3 · last nudge 4m`. */
export function supervisorLine(watching: number | null, on: boolean, lastAt: number | null, nowMs: number, age: (ms: number, now: number) => string): string {
  if (!on) return "supervisor off";
  const parts = [`supervisor watching ${watching ?? "…"}`];
  if (lastAt !== null) parts.push(`last nudge ${age(lastAt * 1000, nowMs)}`);
  return parts.join(" · ");
}
