// The repository wiki (backlog #210): what its buttons say. Which repositories
// it offers is the daemon's one list (lib/repos.ts).

import type { WikiListing } from "../proto";

export const staleCount = (l: WikiListing | null) => l?.pages.filter((p) => p.stale).length ?? 0;

/** The line under the picker: how many pages, how many stale. */
export function wikiSummary(l: WikiListing): string {
  const n = l.pages.length;
  const stale = staleCount(l);
  return `${n} page${n === 1 ? "" : "s"}${stale ? `, ${stale} stale` : ""}`;
}

/** What starting an exploration does, for its button and its confirm. */
export function exploreLabel(l: WikiListing, stale: boolean): { button: string; confirm: string } {
  if (stale) {
    const n = staleCount(l);
    return { button: `Refresh ${n} stale`, confirm: `Start an agent to rewrite ${n} stale page${n === 1 ? "" : "s"}?` };
  }
  return l.pages.length === 0
    ? { button: "Map this repo", confirm: "Start an agent to map this repo?" }
    : { button: "Explore again", confirm: "Start an agent to fill the gaps?" };
}
