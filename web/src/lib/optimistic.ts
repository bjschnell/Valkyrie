// Optimistic actions (backlog #185): what the user just did, shown over the lists
// the daemon pushes until the daemon's own lists say the same.

import type { Decision, Item, ItemChange, QueueItem, SessionId } from "../proto";

/** One change the user made that the daemon may not have shown yet. */
export type Change =
  /** A proposal accepted or rejected: it leaves the proposals. */
  | { t: "decided"; project: string; id: number }
  /** A backlog item's fields, as they'll be once the change lands. */
  | { t: "item"; id: number; patch: Partial<Item> }
  /** A backlog item deleted. */
  | { t: "item_gone"; id: number }
  /** A finished session looked at: it leaves the queue. */
  | { t: "seen"; session: SessionId; seq: number };

export interface Lists {
  proposals: Decision[];
  items: Item[];
  queue: QueueItem[];
}

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

const hidden = (q: QueueItem, c: Change) => c.t === "seen" && q.session === c.session && q.status.seq <= c.seq;

/** `raw` with `changes` applied, in order. Lists no change touches are kept as
 * they are, so a component reading one doesn't redraw for nothing. */
export function overlay(raw: Lists, changes: Change[]): Lists {
  if (!changes.length) return raw;
  let { proposals, items, queue } = raw;
  for (const c of changes) {
    switch (c.t) {
      case "decided":
        proposals = proposals.filter((d) => !(d.project === c.project && d.id === c.id));
        break;
      case "item":
        items = items.map((i) => (i.id === c.id ? { ...i, ...c.patch } : i));
        break;
      case "item_gone":
        items = items.filter((i) => i.id !== c.id);
        break;
      case "seen":
        queue = queue.filter((q) => !hidden(q, c));
        break;
    }
  }
  return { proposals, items, queue };
}

/** Whether the daemon's lists show `c` already, so it can stop being drawn over them. */
export function landed(raw: Lists, c: Change): boolean {
  switch (c.t) {
    case "decided":
      return !raw.proposals.some((d) => d.project === c.project && d.id === c.id);
    case "item": {
      const item = raw.items.find((i) => i.id === c.id);
      if (!item) return true;
      return Object.entries(c.patch).every(([k, v]) => same(item[k as keyof Item], v));
    }
    case "item_gone":
      return !raw.items.some((i) => i.id === c.id);
    case "seen":
      return !raw.queue.some((q) => hidden(q, c));
  }
}

/** An edit as the item's fields it sets, the way the daemon keeps them: text
 * trimmed, the after-list without repeats and sorted. */
export function editPatch(change: Extract<ItemChange, { do: "edit" }>): Partial<Item> {
  const patch: Partial<Item> = {};
  if (change.title != null) patch.title = change.title.trim();
  if (change.body != null) patch.body = change.body.trim();
  if (change.kind != null) patch.kind = change.kind;
  if (change.cwd !== undefined) patch.project = change.cwd ?? undefined;
  if (change.priority != null) patch.priority = change.priority;
  if (change.blocked_by !== undefined) patch.blocked_by = [...new Set(change.blocked_by)].sort((a, b) => a - b);
  if (change.parent !== undefined) patch.parent = change.parent || undefined;
  return patch;
}

/** What the toast says a status change did. */
export function statusDone(was: Item["status"] | undefined, to: Item["status"], id: number): string {
  if (to === "open") return was === "proposed" ? `Accepted #${id}` : `Reopened #${id}`;
  if (to === "shipped") return `Shipped #${id}`;
  if (to === "dropped") return `Dropped #${id}`;
  return `Moved #${id} to ${to}`;
}
