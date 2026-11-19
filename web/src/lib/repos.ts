// The daemon's one list of repositories (backlog #240), as pickers show it.

import type { RepoList } from "../proto";
import { base } from "./backlog";

export interface RepoChoice {
  repo: string;
  label: string;
}

/** The repositories to pick from, by name: the full path where two share one,
 * sorted by name. `keep` (the one already picked) stays offered even when the
 * list hasn't got it, so a select never loses its value. */
export function repoChoices(list: RepoList | null, keep?: string | null): RepoChoice[] {
  const paths = (list?.repos ?? []).map((r) => r.path);
  if (keep && !paths.includes(keep)) paths.push(keep);
  const named = paths.map((repo) => ({ repo, label: base(repo) }));
  const clash = (label: string) => named.filter((n) => n.label === label).length > 1;
  const out = named.map((n) => (clash(n.label) ? { ...n, label: n.repo } : n));
  out.sort((a, b) => a.label.localeCompare(b.label) || a.repo.localeCompare(b.repo));
  return out;
}

/** Whether `path` is one of the folders scanned. */
export const isScanned = (list: RepoList | null, path: string) =>
  list?.folders.some((f) => f.path === path) ?? false;

/** Whether `path` is offered already. */
export const isKnown = (list: RepoList | null, path: string) =>
  list?.repos.some((r) => r.path === path) ?? false;

/** Which repository adding `typed` gave: the daemon keeps a worktree's main
 * checkout, resolved, so it may not read as typed. The one added by hand that
 * wasn't before, else the one that reads as typed. */
export function addedRepo(before: RepoList | null, after: RepoList, typed: string): string | null {
  const was = new Set((before?.repos ?? []).filter((r) => r.added).map((r) => r.path));
  const fresh = after.repos.find((r) => r.added && !was.has(r.path));
  return (fresh ?? after.repos.find((r) => r.path === typed || r.label === typed))?.path ?? null;
}
