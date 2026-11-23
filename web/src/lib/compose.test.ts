import { describe, expect, it } from "vitest";
import { command, findItems, guessRepo, intent, lastRan, readyItems, slug, supervisorLine } from "./compose";
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

const session = (id: number, state: AgentState, over: Partial<SessionInfo> = {}): SessionInfo => ({
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
  ...over,
});

describe("intent", () => {
  it("reads blank as empty, one line as a task, # as a search", () => {
    expect(intent("  \n ")).toEqual({ kind: "empty" });
    expect(intent(" fix the thing ")).toEqual({ kind: "task", text: "fix the thing" });
    expect(intent("#rate limit")).toEqual({ kind: "find", query: "rate limit" });
  });

  it("reads several lines as a batch, blank lines dropped", () => {
    expect(intent("one\n\ntwo\n")).toEqual({ kind: "batch", lines: ["one", "two"] });
  });
});

describe("guessRepo", () => {
  const repos = ["/r/api", "/r/web", "/r/web-admin"];

  it("takes a repo the text names as a word", () => {
    expect(guessRepo("bump axum in api", repos, "/r/web")).toBe("/r/api");
    expect(guessRepo("Web: badge count", repos, "/r/api")).toBe("/r/web");
  });

  it("prefers the longest name, and doesn't match inside a word", () => {
    expect(guessRepo("web-admin login", repos, null)).toBe("/r/web-admin");
    expect(guessRepo("rapid webhooks", repos, "/r/api")).toBe("/r/api");
  });

  it("falls back to the last one used, then the first offered", () => {
    expect(guessRepo("something", repos, "/r/web")).toBe("/r/web");
    expect(guessRepo("something", repos, "/gone")).toBe("/r/api");
    expect(guessRepo("something", [], "/gone")).toBe("/gone");
  });
});

describe("lastRan", () => {
  it("uses what the composer last started there first", () => {
    const items = [item({ launch: { agent: "codex" } })];
    expect(lastRan("/r/api", { "/r/api": { agent: "claude", model: "opus" } }, items, [])).toEqual({ agent: "claude", model: "opus" });
  });

  it("then the newest item launch in the repo", () => {
    const items = [
      item({ id: 1, updated: 5, launch: { agent: "claude", model: "sonnet" } }),
      item({ id: 2, updated: 9, launch: { agent: "codex", model: "gpt-5" } }),
      item({ id: 3, updated: 20, project: "/r/web", launch: { agent: "claude" } }),
    ];
    expect(lastRan("/r/api", {}, items, [])).toEqual({ agent: "codex", model: "gpt-5" });
  });

  it("then a live session's agent in the repo, else claude", () => {
    const codex = session(1, "working", { status: { ...session(1, "working").status, agent: "codex" } });
    expect(lastRan("/r/api", {}, [], [codex])).toEqual({ agent: "codex" });
    expect(lastRan("/r/web", {}, [], [codex])).toEqual({ agent: "claude" });
    expect(lastRan(null, {}, [], [])).toEqual({ agent: "claude" });
  });
});

describe("command", () => {
  it("passes the model only when there is one", () => {
    expect(command({ agent: "claude" }, "go")).toEqual(["claude", "go"]);
    expect(command({ agent: "claude", model: "opus" }, "go")).toEqual(["claude", "--model", "opus", "go"]);
  });
});

describe("slug", () => {
  it("is the first few words in lower case, dashed", () => {
    expect(slug("Add a --json flag to valk doc list")).toBe("add-a-json-flag-to-valk");
    expect(slug("!!!")).toBe("task");
  });

  it("stays short", () => {
    expect(slug("supercalifragilistic expialidocious antidisestablishmentarianism").length).toBeLessThanOrEqual(40);
  });
});

describe("findItems", () => {
  const items = [
    item({ id: 1, number: 4, title: "Rate-limit retries in the client", project: "/r/web" }),
    item({ id: 2, number: 2, title: "Rate-limit the public API", sessions: [9] }),
    item({ id: 3, number: 1, title: "Pin the runner" }),
    item({ id: 4, number: 5, title: "Rate-limit the CI runners", blocked_by: [3] }),
    item({ id: 5, number: 6, title: "Rate-limit nothing", status: "shipped" }),
  ];
  const sessions = [session(9, "blocked")];

  it("matches title words across repos, open items only", () => {
    const got = findItems("rate", items, sessions, 7).map((f) => f.item.id);
    expect(got.sort()).toEqual([1, 2, 4]);
  });

  it("says what picking each does", () => {
    const got = Object.fromEntries(findItems("rate", items, sessions, 7).map((f) => [f.item.id, f]));
    expect(got[1].pick).toBe("start");
    expect(got[2].pick).toBe("open");
    expect(got[2].session?.id).toBe(9);
    expect(got[4].pick).toBe("blocked");
    expect(got[4].why).toBe("after api#1");
  });

  it("matches a number, with or without its repo", () => {
    expect(findItems("4", items, sessions, 7).map((f) => f.item.id)).toEqual([1]);
    expect(findItems("api#1", items, sessions, 7).map((f) => f.item.id)).toEqual([3]);
    expect(findItems("web#1", items, sessions, 7)).toEqual([]);
  });

  it("can't start an item with no repo", () => {
    const f = findItems("loose", [item({ title: "loose end", project: undefined })], [], 7)[0];
    expect(f.pick).toBe("blocked");
    expect(f.why).toBe("no repo");
  });
});

describe("readyItems", () => {
  it("is the top unblocked, untouched items in work order", () => {
    const items = [
      item({ id: 1, priority: "p3" }),
      item({ id: 2, priority: "p1" }),
      item({ id: 3, sessions: [1] }),
      item({ id: 4, blocked_by: [1] }),
      item({ id: 5, project: undefined }),
      item({ id: 6, pr: "https://x/pull/1" }),
      item({ id: 7, status: "proposed" }),
      item({ id: 8, priority: "p0" }),
      item({ id: 9 }),
    ];
    expect(readyItems(items, [], 7, new Set()).map((i) => i.id)).toEqual([8, 2, 9]);
  });

  it("leaves out what's already on the screen", () => {
    const items = [item({ id: 1 }), item({ id: 2 })];
    expect(readyItems(items, [], 7, new Set([1])).map((i) => i.id)).toEqual([2]);
  });
});

describe("supervisorLine", () => {
  const age = (ms: number, now: number) => `${Math.round((now - ms) / 60000)}m`;
  it("says how many it watches and the last nudge", () => {
    expect(supervisorLine(3, true, 60, 60_000 + 180_000, age)).toBe("supervisor watching 3 · last nudge 3m");
    expect(supervisorLine(0, true, null, 0, age)).toBe("supervisor watching 0");
    expect(supervisorLine(2, false, 60, 0, age)).toBe("supervisor off");
  });
});
