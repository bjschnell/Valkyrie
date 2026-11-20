import { describe, expect, it } from "vitest";
import { attention, filedLine, sessionKind, type Waiting } from "./attention";
import type { AgentState, Decision, Item, LaunchOffer, OrderProposal, QueueItem } from "../proto";

const q = (session: number, state: AgentState, since: number, flag = false): QueueItem =>
  ({
    session,
    name: `s${session}`,
    cwd: "/r/web",
    status: { agent: "claude", state, since_ms: since, seq: 1, summary: null },
    ...(flag ? { flag: { seq: 1, reason: "CI red" } } : {}),
  }) as unknown as QueueItem;

const decision = (id: number, created: number) =>
  ({ id, project: "/r/api", kind: "decision", status: "proposed", title: `d${id}`, created }) as unknown as Decision;
const offer = (id: number, made: number) => ({ id, project: "/r/api", item: 1, number: 1, title: "t", agent: "claude", why: "", made }) as LaunchOffer;
const order = (id: number, made: number) => ({ id, project: "/r/web", made, trigger: "you asked", steps: [] }) as OrderProposal;
const item = (id: number, project: string | undefined, created: number, more: Partial<Item> = {}) =>
  ({ id, number: id, title: `i${id}`, status: "proposed", project, created, by: "claude", ...more }) as unknown as Item;

const list = (w: Partial<Waiting>) =>
  attention({ queue: [], proposals: [], launches: [], orders: [], items: [], ...w });

describe("sessionKind", () => {
  it("files a session by what it waits on", () => {
    expect(sessionKind(q(1, "needs_input", 0))).toBe("ask");
    expect(sessionKind(q(1, "needs_input", 0, true))).toBe("ask");
    expect(sessionKind(q(1, "blocked", 0))).toBe("stuck");
    expect(sessionKind(q(1, "stale", 0))).toBe("stuck");
    expect(sessionKind(q(1, "review_ready", 0, true))).toBe("stuck");
    expect(sessionKind(q(1, "review_ready", 0))).toBe("done");
    expect(sessionKind(q(1, "idle", 0))).toBe("done");
  });
});

describe("attention", () => {
  it("orders Ask, Stuck, Done, Decide, Filed, oldest first within each", () => {
    const got = list({
      queue: [q(1, "review_ready", 5), q(2, "needs_input", 9), q(3, "blocked", 1), q(4, "needs_input", 3)],
      proposals: [decision(7, 2)],
      launches: [offer(1, 1)],
      orders: [order(1, 3)],
      items: [item(10, "/r/web", 0)],
    });
    expect(got.map((a) => a.key)).toEqual(["s4", "s2", "s3", "s1", "l/r/api:1", "d/r/api#7", "o/r/web:1", "f/r/web"]);
  });

  it("batches agent-filed items per repository, one row each", () => {
    const got = list({
      items: [item(1, "/r/api", 5), item(2, "/r/web", 1), item(3, "/r/api", 2), item(4, undefined, 9)],
    });
    expect(got.map((a) => a.kind === "filed" && [a.project, a.items.map((i) => i.id)])).toEqual([
      ["/r/web", [2]],
      ["/r/api", [1, 3]],
      [null, [4]],
    ]);
  });

  it("leaves out items already kept, and an agent's own filed-and-taken work", () => {
    const got = list({
      items: [item(1, "/r/api", 0, { status: "open" }), item(2, "/r/api", 0, { filed_by: [1, 2], taken: [2] })],
    });
    expect(got).toEqual([]);
  });
});

describe("filedLine", () => {
  it("says how many and where", () => {
    expect(filedLine({ project: "/r/api", items: [item(1, "/r/api", 0), item(2, "/r/api", 0)] })).toBe("2 items filed · api");
    expect(filedLine({ project: null, items: [item(1, undefined, 0)] })).toBe("1 item filed · no repository");
  });
});
