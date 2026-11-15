import { describe, expect, it } from "vitest";
import { editPatch, landed, overlay, statusDone, type Lists } from "./optimistic";
import type { Decision, Item, QueueItem } from "../proto";

const item = (id: number, over: Partial<Item> = {}): Item => ({
  id,
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

const decision = (project: string, id: number) => ({ project, id }) as Decision;

const queued = (session: number, seq: number) =>
  ({
    session,
    name: "s",
    cwd: "/r",
    status: { agent: "claude", state: "review_ready", ask: null, summary: null, since_ms: 0, seq, seen: false, hooked: true },
  }) as QueueItem;

const raw = (): Lists => ({
  proposals: [decision("/a", 1), decision("/b", 1)],
  items: [item(1), item(2, { title: "two" })],
  queue: [queued(5, 3), queued(6, 1)],
});

describe("overlay", () => {
  it("leaves the lists as they are with nothing pending", () => {
    const r = raw();
    expect(overlay(r, [])).toBe(r);
  });

  it("drops a decided proposal of that project only", () => {
    const out = overlay(raw(), [{ t: "decided", project: "/a", id: 1 }]);
    expect(out.proposals).toEqual([decision("/b", 1)]);
  });

  it("keeps the lists a change doesn't touch", () => {
    const r = raw();
    const out = overlay(r, [{ t: "item_gone", id: 1 }]);
    expect(out.proposals).toBe(r.proposals);
    expect(out.queue).toBe(r.queue);
    expect(out.items.map((i) => i.id)).toEqual([2]);
  });

  it("patches an item, later changes over earlier ones", () => {
    const out = overlay(raw(), [
      { t: "item", id: 2, patch: { status: "shipped", title: "x" } },
      { t: "item", id: 2, patch: { status: "open" } },
    ]);
    expect(out.items[1]).toMatchObject({ status: "open", title: "x" });
    expect(out.items[0]).toEqual(item(1));
  });

  it("hides a seen session until a newer status comes", () => {
    const r = raw();
    expect(overlay(r, [{ t: "seen", session: 5, seq: 3 }]).queue.map((q) => q.session)).toEqual([6]);
    r.queue[0] = queued(5, 4);
    expect(overlay(r, [{ t: "seen", session: 5, seq: 3 }]).queue.map((q) => q.session)).toEqual([5, 6]);
  });
});

describe("landed", () => {
  it("knows a decision landed once it's off the proposals", () => {
    const c = { t: "decided", project: "/a", id: 1 } as const;
    expect(landed(raw(), c)).toBe(false);
    expect(landed({ ...raw(), proposals: [decision("/b", 1)] }, c)).toBe(true);
  });

  it("knows an item change landed once its fields match", () => {
    const c = { t: "item", id: 2, patch: { title: "new", blocked_by: [1, 3] } } as const;
    expect(landed(raw(), c)).toBe(false);
    const r = raw();
    r.items[1] = item(2, { title: "new", blocked_by: [1, 3] });
    expect(landed(r, c)).toBe(true);
    // Gone is as good: nothing left to draw over.
    expect(landed({ ...raw(), items: [item(1)] }, c)).toBe(true);
  });

  it("treats an absent field as matching an unset one", () => {
    expect(landed(raw(), { t: "item", id: 1, patch: { project: undefined } })).toBe(true);
  });

  it("knows a delete and a seen landed", () => {
    expect(landed(raw(), { t: "item_gone", id: 1 })).toBe(false);
    expect(landed({ ...raw(), items: [] }, { t: "item_gone", id: 1 })).toBe(true);
    expect(landed(raw(), { t: "seen", session: 5, seq: 3 })).toBe(false);
    expect(landed({ ...raw(), queue: [] }, { t: "seen", session: 5, seq: 3 })).toBe(true);
  });
});

describe("editPatch", () => {
  it("sets only what the edit changes, as the daemon stores it", () => {
    expect(editPatch({ do: "edit", title: " t ", cwd: "/r", blocked_by: [3, 1, 3] })).toEqual({
      title: "t",
      project: "/r",
      blocked_by: [1, 3],
    });
    expect(editPatch({ do: "edit" })).toEqual({});
  });
});

describe("statusDone", () => {
  it("says what the status change did", () => {
    expect(statusDone("proposed", "open", 4)).toBe("Accepted #4");
    expect(statusDone("shipped", "open", 4)).toBe("Reopened #4");
    expect(statusDone("open", "shipped", 4)).toBe("Shipped #4");
    expect(statusDone("open", "dropped", 4)).toBe("Dropped #4");
  });
});
