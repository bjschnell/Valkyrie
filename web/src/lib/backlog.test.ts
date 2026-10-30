import { describe, expect, it } from "vitest";
import { byColumn, byLane, editChange, factsLine, lane, liveSession, prLink, shippedSummary, wouldShip } from "./backlog";
import type { AgentState, Item, SessionInfo } from "../proto";

const item = (over: Partial<Item> = {}): Item => ({
  id: 1,
  title: "t",
  body: "",
  kind: "bug",
  status: "open",
  by: "human",
  boot: 7,
  created: 0,
  updated: 0,
  ...over,
});

const session = (id: number, state: AgentState, exited: number | null = null): SessionInfo => ({
  id,
  name: `s${id}`,
  command: ["claude"],
  cwd: "/r",
  pid: 1,
  created_unix: 0,
  title: null,
  clients: 0,
  exited,
  status: { agent: "claude", state, ask: null, summary: null, since_ms: 0, seq: 0, seen: false, hooked: true },
});

describe("lane", () => {
  it("is the backlog until something is started on it", () => {
    expect(lane(item(), [], 7)).toBe("backlog");
  });

  it("follows the newest live session's state", () => {
    const i = item({ sessions: [3, 4] });
    expect(lane(i, [session(3, "needs_input"), session(4, "working")], 7)).toBe("running");
    expect(lane(i, [session(4, "needs_input")], 7)).toBe("needs");
    expect(lane(i, [session(4, "blocked")], 7)).toBe("needs");
    expect(lane(i, [session(4, "review_ready")], 7)).toBe("review");
    // The newest one exited: the older one still running counts.
    expect(liveSession(i, [session(3, "working"), session(4, "working", 0)], 7)?.id).toBe(3);
  });

  it("waits on review once its sessions are gone, or belong to another daemon", () => {
    const i = item({ sessions: [3], checkout: "/r-fix-1-t" });
    expect(lane(i, [], 7)).toBe("review");
    // Session 3 of a restarted daemon is someone else's.
    expect(lane(i, [session(3, "working")], 8)).toBe("review");
    expect(liveSession(i, [session(3, "working")], 8)).toBeNull();
  });

  it("puts shipped and dropped items in done, whatever runs", () => {
    expect(lane(item({ status: "shipped", sessions: [3] }), [session(3, "working")], 7)).toBe("done");
    expect(lane(item({ status: "dropped" }), [], 7)).toBe("done");
  });
});

describe("byLane", () => {
  it("sorts each lane newest first", () => {
    const lanes = byLane([item({ id: 1, updated: 5 }), item({ id: 2, updated: 9 }), item({ id: 3, status: "shipped" })], [], 7);
    expect(lanes.backlog.map((i) => i.id)).toEqual([2, 1]);
    expect(lanes.done.map((i) => i.id)).toEqual([3]);
  });
});

describe("byColumn", () => {
  it("puts one that needs you at the top of in progress", () => {
    const working = item({ id: 1, sessions: [3], updated: 9 });
    const stuck = item({ id: 2, sessions: [4], updated: 1 });
    const cols = byColumn([working, stuck, item({ id: 5 })], [session(3, "working"), session(4, "needs_input")], 7);
    expect(cols.progress.map(({ item, lane }) => [item.id, lane])).toEqual([
      [2, "needs"],
      [1, "running"],
    ]);
    expect(cols.backlog.map(({ item }) => item.id)).toEqual([5]);
    expect(cols.review).toEqual([]);
  });

  it("puts what an agent proposed at the top of the backlog", () => {
    const cols = byColumn([item({ id: 1, updated: 9 }), item({ id: 2, status: "proposed", by: "claude", updated: 1 })], [], 7);
    expect(cols.backlog.map(({ item, lane }) => [item.id, lane])).toEqual([
      [2, "proposed"],
      [1, "backlog"],
    ]);
  });
});

describe("factsLine", () => {
  it("says only what there is", () => {
    expect(factsLine({ branch: "fix/1-x", ahead: 0, commits: [], diff: "", dirty: 0 })).toBe("0 commits");
    expect(
      factsLine({ branch: "fix/1-x", ahead: 1, commits: ["abc x"], diff: "1 file changed, 2 insertions(+)", dirty: 3 }),
    ).toBe("1 commit · 1 file changed, 2 insertions(+) · 3 not committed");
  });
});

describe("editChange", () => {
  const was = item({ title: "old", body: "detail", project: "/r/a" });
  const mine = { title: "old", body: "detail", kind: "bug" as const, project: "/r/a" };

  it("sends only what was changed here", () => {
    expect(editChange(was, was, mine)).toBeNull();
    expect(editChange(was, was, { ...mine, title: "new" })).toEqual({ do: "edit", title: "new" });
    expect(editChange(was, was, { ...mine, kind: "chore", project: "/r/b" })).toEqual({
      do: "edit",
      kind: "chore",
      cwd: "/r/b",
    });
  });

  it("keeps a change made elsewhere to a field not touched here", () => {
    const now = { ...was, body: "rewritten on the phone" };
    expect(editChange(was, now, { ...mine, title: "new" })).toEqual({ do: "edit", title: "new" });
  });

  it("refuses a field changed both here and elsewhere", () => {
    const now = { ...was, body: "rewritten on the phone" };
    const clash = editChange(was, now, { ...mine, body: "mine" });
    expect(clash).toMatchObject({ clash: expect.stringMatching(/body changed elsewhere/) });
    // Saving again, from what's there now, replaces it.
    const seen = (clash as { was: Item }).was;
    expect(editChange(seen, now, { ...mine, body: "mine" })).toEqual({ do: "edit", body: "mine" });
    // Whitespace the daemon trims away is no change, and no clash.
    expect(editChange(was, was, { ...mine, body: "detail\n" })).toBeNull();
    expect(editChange(was, now, { ...mine, body: "rewritten on the phone " })).toEqual({
      do: "edit",
      body: "rewritten on the phone ",
    });
    // The same text on both sides isn't a clash.
    expect(editChange(was, now, { ...mine, body: "rewritten on the phone" })).toEqual({
      do: "edit",
      body: "rewritten on the phone",
    });
  });

  it("after a clash, still leaves alone what changed elsewhere and not here", () => {
    const now = { ...was, title: "A2", body: "B2" };
    const first = editChange(was, now, { ...mine, body: "B3" });
    expect(first).toMatchObject({ clash: expect.stringMatching(/'s body changed/) });
    const seen = (first as { was: Item }).was;
    expect(seen.title).toBe("old");
    expect(editChange(seen, now, { ...mine, body: "B3" })).toEqual({ do: "edit", body: "B3" });
  });
});

describe("prLink", () => {
  it("links only a plain https URL", () => {
    expect(prLink("https://github.com/o/r/pull/7")).toBe("https://github.com/o/r/pull/7");
    expect(prLink(undefined)).toBeNull();
    expect(prLink("http://github.com/o/r/pull/7")).toBeNull();
    expect(prLink("javascript:alert(1)")).toBeNull();
    expect(prLink("https://")).toBeNull();
    expect(prLink("https://x/ y")).toBeNull();
  });
});

describe("shippedSummary", () => {
  it("names what shipped and counts what didn't, as of the last fetch", () => {
    const shipped = [1, 4].map((id) => ({ id, title: "", branch: `fix/${id}`, against: "origin/main" }));
    const kept = [
      { id: 2, title: "", why: "not merged" },
      { id: 3, title: "", why: "not merged" },
      { id: 9, title: "", why: "branch fix/x not found" },
    ];
    expect(shippedSummary(shipped, kept)).toBe(
      "Shipped #1 #4 · 2 not merged · #9 branch fix/x not found (as of the last fetch)",
    );
    expect(shippedSummary([], [])).toBe("Nothing merged to ship (as of the last fetch)");
  });
});

describe("wouldShip", () => {
  it("names each branch, marking one a take set", () => {
    const shipped = [
      { id: 3, title: "", branch: "fix/old", branch_taken: true, against: "origin/main" },
      { id: 4, title: "", branch: "fix/4", against: "origin/main" },
    ];
    expect(wouldShip(shipped, [{ id: 5, title: "", why: "not merged" }])).toBe(
      "Tap again to ship #3 fix/old (set by a take), merged into origin/main · #4 fix/4, merged into origin/main" +
        " · 1 not merged (as of the last fetch)",
    );
  });
});
