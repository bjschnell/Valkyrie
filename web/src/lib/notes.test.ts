import { describe, expect, it } from "vitest";
import type { Note } from "../proto";
import { notesByProject } from "./notes";

const n = (id: number, more: Partial<Note> = {}): Note => ({
  id,
  project: "/r/app",
  text: `note ${id}`,
  status: "live",
  created: id,
  by: "agent",
  session: "none",
  ...more,
});

describe("notesByProject", () => {
  it("lists live notes newest first with stale ones last", () => {
    const groups = notesByProject([n(1), n(2, { stale: "gone" }), n(3), n(4, { status: "replaced" })]);
    expect(groups.map(([, ns]) => ns.map((x) => x.id))).toEqual([[3, 1, 2]]);
  });

  it("lists the replaced and promoted ones apart, by project name", () => {
    const groups = notesByProject(
      [n(1, { project: "/r/zeta", status: "promoted" }), n(2, { project: "/r/alpha", status: "replaced" }), n(3)],
      true,
    );
    expect(groups.map(([p, ns]) => [p, ns.map((x) => x.id)])).toEqual([
      ["/r/alpha", [2]],
      ["/r/zeta", [1]],
    ]);
  });
});
