import { describe, expect, it } from "vitest";
import { draft, namedItem, pickable, rowsFrom, spoken, summary, type Context } from "./batch";
import type { AgentState, Item, SessionInfo } from "../proto";

const item = (over: Partial<Item> = {}): Item => ({
  id: 1,
  number: 1,
  title: "t",
  body: "",
  kind: "bug",
  status: "open",
  by: "human",
  boot: 7,
  project: "/r/api",
  created: 0,
  updated: 0,
  ...over,
});

const session = (id: number, state: AgentState): SessionInfo => ({
  id,
  name: `s${id}`,
  command: ["claude"],
  cwd: "/r/api",
  pid: 1,
  created_unix: 0,
  title: null,
  clients: 0,
  exited: null,
  status: { agent: "claude", state, ask: null, summary: null, since_ms: 0, seq: 0, seen: false, hooked: true },
});

const ctx = (over: Partial<Context> = {}): Context => ({
  repos: ["/r/api", "/r/web"],
  last: "/r/web",
  remembered: {},
  items: [],
  sessions: [],
  boot: 7,
  ...over,
});

describe("rowsFrom", () => {
  it("gives each line the repo it names, else the last one used", () => {
    const rows = rowsFrom("Fix the refill test in api\n\nRelease notes\n", ctx());
    expect(rows.map((r) => [r.task, r.repo, r.item])).toEqual([
      ["Fix the refill test in api", "/r/api", null],
      ["Release notes", "/r/web", null],
    ]);
  });

  it("starts each on what its repo last ran", () => {
    const rows = rowsFrom("bump api\nweb thing", ctx({ remembered: { "/r/api": { agent: "codex" } } }));
    expect(rows.map((r) => r.ran)).toEqual([{ agent: "codex" }, { agent: "claude" }]);
  });

  it("reads a backlog number as that item, its repo fixed", () => {
    const items = [item({ id: 40, number: 3, title: "Rate-limit the API", launch: { agent: "codex", model: "gpt-5" } })];
    const [row] = rowsFrom("api#3", ctx({ items }));
    expect(row).toMatchObject({ item: 40, repo: "/r/api", task: "Rate-limit the API", ran: { agent: "codex", model: "gpt-5" }, why: null });
  });

  it("says why an item can't start", () => {
    const items = [item({ id: 1, number: 1 }), item({ id: 2, number: 2, blocked_by: [1] }), item({ id: 3, number: 3, sessions: [9] })];
    const rows = rowsFrom("api#2\napi#3", ctx({ items, sessions: [session(9, "working")] }));
    expect(rows.map((r) => r.why)).toEqual(["after api#1", "has a session"]);
  });

  it("has no repo to start in when none is known", () => {
    expect(rowsFrom("anything", ctx({ repos: [], last: null }))[0].why).toBe("no repo");
  });
});

describe("namedItem", () => {
  const items = [item({ id: 1, number: 7 }), item({ id: 2, number: 7, project: "/r/web" }), item({ id: 3, number: 8, status: "shipped" })];

  it("takes the repo's own number", () => {
    expect(namedItem("web#7", items, null)?.id).toBe(2);
    expect(namedItem("API#7", items, null)?.id).toBe(1);
  });

  it("reads a bare number in the last repo used, when several have it", () => {
    expect(namedItem("#7", items, "/r/api")?.id).toBe(1);
    expect(namedItem("#7", items, null)).toBeNull();
  });

  it("names only open items, and only whole lines", () => {
    expect(namedItem("#8", items, null)).toBeNull();
    expect(namedItem("fix #7", items, "/r/api")).toBeNull();
  });
});

describe("spoken", () => {
  it("makes each pause a task", () => {
    expect(spoken("fix the login\nwrite release notes")).toEqual(["Fix the login", "Write release notes"]);
  });

  it("splits on a spoken next, but not on next inside a task", () => {
    expect(spoken("fix the login. Next, write notes\nnext cache the build")).toEqual(["Fix the login", "Write notes", "Cache the build"]);
    expect(spoken("plan the next release")).toEqual(["Plan the next release"]);
  });
});

describe("pickable", () => {
  it("lists ready items first, then blocked ones, leaving out what's taken", () => {
    const items = [
      item({ id: 1, priority: "p1" }),
      item({ id: 2, priority: "p0", blocked_by: [1] }),
      item({ id: 3, sessions: [4] }),
      item({ id: 4, pr: "https://x/pull/1" }),
      item({ id: 5, project: undefined }),
      item({ id: 6 }),
      item({ id: 7 }),
    ];
    expect(pickable(items, [], 7, new Set([7])).map((f) => [f.item.id, f.pick])).toEqual([
      [1, "start"],
      [6, "start"],
      [2, "blocked"],
    ]);
  });
});

describe("summary and draft", () => {
  it("counts agents and repos", () => {
    const rows = rowsFrom("a in api\nb in api\nc", ctx());
    expect(summary(rows)).toBe("3 agents · 2 repos");
    expect(summary(rows.slice(0, 1))).toBe("1 agent · 1 repo");
  });

  it("gives typed lines back as typed, items by number", () => {
    const items = [item({ id: 40, number: 3, title: "Rate-limit" })];
    const c = ctx({ items });
    expect(draft(rowsFrom("first\napi#3", c), items)).toBe("first\napi#3");
  });
});
