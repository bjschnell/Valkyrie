import type { Item } from "../proto";
import { agentFiled } from "./backlog";

/** Agents' new backlog proposals pop up as a toast (#194); this browser's choice. */
const POPUPS_KEY = "valk.proposalPopups";

export function proposalPopups(): boolean {
  try {
    return localStorage.getItem(POPUPS_KEY) !== "off";
  } catch {
    return true;
  }
}

export function setProposalPopups(on: boolean): void {
  try {
    if (on) localStorage.removeItem(POPUPS_KEY);
    else localStorage.setItem(POPUPS_KEY, "off");
  } catch {
    // Private mode: it holds for this page only.
  }
}

/**
 * The proposals in `items` new since `known` (the ids seen last time; `null` before
 * the first push, which only primes: what was waiting on connecting isn't news).
 * An item that comes back to proposed (reopened unaccepted) isn't new, nor is one
 * an agent filed and took for its own work: that one follows its session.
 */
export function newProposals(known: Set<number> | null, items: Item[]): { known: Set<number>; fresh: Item[] } {
  const fresh =
    known === null
      ? []
      : items.filter((i) => !known.has(i.id) && i.status === "proposed" && i.by !== "human" && !agentFiled(i));
  return { known: new Set(items.map((i) => i.id)), fresh };
}

/** What the toast says, and the proposal it answers: the newest one. */
export function proposalToast(fresh: Item[]): { text: string; id: number } | null {
  if (fresh.length === 0) return null;
  const newest = fresh.reduce((a, b) => (b.id > a.id ? b : a));
  const text =
    fresh.length === 1
      ? `${newest.by} proposed #${newest.number} ${newest.title}`
      : `${fresh.length} agent proposals, newest #${newest.number} ${newest.title}`;
  return { text, id: newest.id };
}
