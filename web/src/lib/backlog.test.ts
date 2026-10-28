import { describe, expect, it } from "vitest";
import { byColumn, byLane, lane, liveSession } from "./backlog";
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
