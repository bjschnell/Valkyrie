import { describe, expect, it } from "vitest";
import type { Decision, DecisionKind } from "../proto";
import { byProject, inFull, pinnable } from "./decisions";
import { USER_SCOPE } from "./scope";

const d = (id: number, kind: DecisionKind, more: Partial<Decision> = {}): Decision => ({
  id,
  project: "/r/app",
  title: `Title ${id}`,
  body: "",
  kind,
  status: "active",
  created: 0,
  updated: id,
  provenance: { by: "human" },
  ...more,
});

describe("byProject", () => {
  it("puts what's in full first, then ranks as the block does, and skips inactive ones", () => {
    const groups = byProject([
      d(1, "pattern"),
      d(2, "gotcha"),
      d(3, "decision", { pinned: true }),
      d(4, "constraint"),
      d(5, "gotcha", { status: "retired" }),
    ]);
    expect(groups.map(([, ds]) => ds.map((x) => x.id))).toEqual([[4, 3, 2, 1]]);
  });

  it("orders projects by name with your habits last", () => {
    const groups = byProject([
      d(1, "decision", { project: USER_SCOPE }),
      d(2, "decision", { project: "/r/zeta" }),
      d(3, "decision", { project: "/r/alpha" }),
    ]);
    expect(groups.map(([p]) => p)).toEqual(["/r/alpha", "/r/zeta", USER_SCOPE]);
  });
});

describe("pinnable", () => {
  it("is only project decisions that don't already have or never have a body in the block", () => {
    expect(pinnable(d(1, "gotcha"))).toBe(true);
    expect(pinnable(d(1, "constraint"))).toBe(false);
    expect(pinnable(d(1, "skill"))).toBe(false);
    expect(pinnable(d(1, "policy"))).toBe(false);
    expect(pinnable(d(1, "decision", { project: USER_SCOPE }))).toBe(false);
    expect(inFull(d(1, "pattern", { pinned: true }))).toBe(true);
  });
});
