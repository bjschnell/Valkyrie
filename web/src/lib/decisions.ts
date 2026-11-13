import type { Decision, DecisionKind } from "../proto";
import { USER_SCOPE, scopeName } from "./scope";

/** As the session-start block ranks them (`inject::rank`). */
const RANK: Record<DecisionKind, number> = {
  constraint: 0,
  gotcha: 1,
  decision: 2,
  pattern: 3,
  fix: 4,
  policy: 5,
  skill: 6,
};

/** Whether its body goes in every session's block (ADR-0014 §1). */
export const inFull = (d: Decision) => d.kind === "constraint" || !!d.pinned;

/** Whether you can pin it: a constraint always has its body, a skill or policy
 * never, and habits are listed by title. */
export const pinnable = (d: Decision) =>
  d.project !== USER_SCOPE && !["constraint", "skill", "policy"].includes(d.kind);

/** Globs typed into a scope field, split on spaces, commas and new lines. */
export const parseScope = (text: string) =>
  text
    .split(/[\s,]+/)
    .map((g) => g.trim())
    .filter(Boolean);

/** Whether `d` waits on you for a scope (ADR-0014 §2): active, with one proposed,
 * and not also flagged as out of date, which comes first. */
export const scoping = (d: Decision) => d.status === "active" && !d.fresh?.review && !!d.proposed_scope?.length;

/** Where the Memory page files a decision: your habits on their own shelf, a
 * project's skills and policies on theirs, and every other kind as decisions. */
export type Shelf = "decisions" | "skills" | "policies" | "habits";
export const SHELVES: Shelf[] = ["decisions", "skills", "policies", "habits"];

export const shelf = (d: Decision): Shelf =>
  d.project === USER_SCOPE ? "habits" : d.kind === "skill" ? "skills" : d.kind === "policy" ? "policies" : "decisions";

/** No longer given to agents: retired, rejected, or replaced by another. Proposals
 * are neither: they wait on you in the inbox. */
export const ended = (d: Decision) => d.status === "retired" || d.status === "rejected" || d.status === "superseded";

/** Whether every word typed into a filter is in its title, body or scope. */
export function matches(words: string, ...texts: (string | undefined)[]): boolean {
  const hay = texts.join("\n").toLowerCase();
  return words
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

/** By project, as `byProject` groups them: active ones (the default), or with
 * `ended`, the ones agents no longer get, most recently changed first. */
export function byProject(list: Decision[], showEnded = false): [string, Decision[]][] {
  const groups = new Map<string, Decision[]>();
  for (const d of list.filter((d) => (showEnded ? ended(d) : d.status === "active"))) {
    groups.set(d.project, [...(groups.get(d.project) ?? []), d]);
  }
  for (const ds of groups.values()) {
    ds.sort(
      showEnded
        ? (a, b) => b.updated - a.updated || b.id - a.id
        : (a, b) =>
            Number(inFull(b)) - Number(inFull(a)) ||
            RANK[a.kind] - RANK[b.kind] ||
            b.updated - a.updated ||
            b.id - a.id,
    );
  }
  // Projects by name, your habits last.
  return [...groups.entries()].sort(([a], [b]) =>
    a === USER_SCOPE ? 1 : b === USER_SCOPE ? -1 : scopeName(a).localeCompare(scopeName(b)),
  );
}
