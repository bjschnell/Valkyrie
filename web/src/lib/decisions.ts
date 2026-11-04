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

/** Active decisions by project, the ones in full first, then as the block ranks them. */
export function byProject(list: Decision[]): [string, Decision[]][] {
  const groups = new Map<string, Decision[]>();
  for (const d of list.filter((d) => d.status === "active")) {
    groups.set(d.project, [...(groups.get(d.project) ?? []), d]);
  }
  for (const ds of groups.values()) {
    ds.sort(
      (a, b) =>
        Number(inFull(b)) - Number(inFull(a)) || RANK[a.kind] - RANK[b.kind] || b.updated - a.updated || b.id - a.id,
    );
  }
  // Projects by name, your habits last.
  return [...groups.entries()].sort(([a], [b]) =>
    a === USER_SCOPE ? 1 : b === USER_SCOPE ? -1 : scopeName(a).localeCompare(scopeName(b)),
  );
}
