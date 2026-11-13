import type { Note } from "../proto";
import { scopeName } from "./scope";

/** Live notes are the ones search serves; replaced and promoted ones are kept
 * only so their chain reads (ADR-0017 §3, §7). */
export const live = (n: Note) => n.status === "live";

/** Notes by project, projects by name: live ones (stale last, then newest
 * first), or with `ended`, the replaced and promoted ones, newest first. */
export function notesByProject(list: Note[], showEnded = false): [string, Note[]][] {
  const groups = new Map<string, Note[]>();
  for (const n of list.filter((n) => live(n) !== showEnded)) {
    groups.set(n.project, [...(groups.get(n.project) ?? []), n]);
  }
  for (const ns of groups.values()) {
    ns.sort((a, b) => Number(!!a.stale) - Number(!!b.stale) || b.created - a.created || b.id - a.id);
  }
  return [...groups.entries()].sort(([a], [b]) => scopeName(a).localeCompare(scopeName(b)));
}
