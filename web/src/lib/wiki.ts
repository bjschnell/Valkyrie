// The repository wiki (backlog #210): which repositories the page offers, and
// what its buttons say.

import type { Item, SessionInfo, WikiListing } from "../proto";
import { base } from "./backlog";

/** The repositories to offer: every backlog item's, then each session's
 * directory that isn't one of those or a worktree of one (`<repo>-<branch>`
 * beside it, as `valk` names them). The daemon resolves any to its root. */
export function wikiRepos(items: Item[], sessions: SessionInfo[]): { repo: string; label: string }[] {
  const roots = [...new Set(items.flatMap((i) => (i.project ? [i.project] : [])))];
  const covered = (dir: string) =>
    roots.some((r) => dir === r || dir.startsWith(`${r}/`) || base(dir).startsWith(`${base(r)}-`));
  const extra = [...new Set(sessions.map((s) => s.cwd))].filter((d) => d && !covered(d));
  const named = [...roots, ...extra].map((repo) => ({ repo, label: base(repo) }));
  const clash = (label: string) => named.filter((n) => n.label === label).length > 1;
  const out = named.map((n) => (clash(n.label) ? { ...n, label: n.repo } : n));
  out.sort((a, b) => a.label.localeCompare(b.label) || a.repo.localeCompare(b.repo));
  return out;
}

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
