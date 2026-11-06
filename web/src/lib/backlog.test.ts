import { describe, expect, it } from "vitest";
import { agentFiled, appliedSummary, byColumn, changesSomething, inRepo, launchesOn, launchesShown, ordersShown, planScope, repoChoices, cardActions, byLane, editChange, factsLine, lane, liveSession, prLink, shippedSummary, startAgent, stepChanges, takenOnly, waitingOn, wouldShip } from "./backlog";
import type { AgentState, Item, LaunchOffer, OrderProposal, SessionInfo } from "../proto";

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

  it("keeps an interrupted session running: nothing in it is complete", () => {
    const i = item({ sessions: [4] });
    expect(lane(i, [session(4, "interrupted")], 7)).toBe("running");
    expect(lane(i, [session(4, "review_ready")], 7)).toBe("review");
    expect(lane({ ...i, pr: "https://github.com/o/r/pull/7" }, [session(4, "interrupted")], 7)).toBe("review");
  });

  it("waits on review once its sessions are gone, or belong to another daemon", () => {
    const i = item({ sessions: [3], checkout: "/r-fix-1-t" });
    expect(lane(i, [], 7)).toBe("review");
    // Session 3 of a restarted daemon is someone else's.
    expect(lane(i, [session(3, "working")], 8)).toBe("review");
    expect(liveSession(i, [session(3, "working")], 8)).toBeNull();
  });

  it("waits on review with a pull request open, unless its session needs you", () => {
    const i = item({ sessions: [4], pr: "https://github.com/o/r/pull/7" });
    expect(lane(i, [session(4, "working")], 7)).toBe("review");
    expect(lane(i, [session(4, "stale")], 7)).toBe("review");
    expect(lane(i, [session(4, "needs_input")], 7)).toBe("needs");
    expect(lane(i, [session(4, "blocked")], 7)).toBe("needs");
    expect(lane(i, [], 7)).toBe("review");
    expect(lane({ ...i, status: "shipped" }, [session(4, "working")], 7)).toBe("done");
    expect(lane({ ...i, status: "proposed" }, [], 7)).toBe("proposed");
  });

  it("follows the session of an agent's own filed work, until it's accepted", () => {
    const filed = item({ status: "proposed", by: "claude", sessions: [4], taken: [4], filed_by: [7, 4] });
    expect(agentFiled(filed)).toBe(true);
    expect(lane(filed, [session(4, "working")], 7)).toBe("running");
    expect(lane(filed, [session(4, "needs_input")], 7)).toBe("needs");
    expect(lane({ ...filed, pr: "https://github.com/o/r/pull/7" }, [session(4, "working")], 7)).toBe("review");
    expect(lane(filed, [], 7)).toBe("review");
    // Proposed without taking it, it waits on you; accepted, it's no longer agent-filed.
    expect(lane({ ...filed, filed_by: undefined }, [session(4, "working")], 7)).toBe("proposed");
    // Filed by the agent but not taken yet: a proposal like any other.
    expect(agentFiled({ ...filed, sessions: [], taken: [] })).toBe(false);
    expect(lane({ ...filed, sessions: [], taken: [] }, [], 7)).toBe("proposed");
    expect(agentFiled({ ...filed, status: "open" })).toBe(false);
    expect(lane({ ...filed, status: "shipped" }, [], 7)).toBe("done");
  });

  it("puts shipped and dropped items in done, whatever runs", () => {
    expect(lane(item({ status: "shipped", sessions: [3] }), [session(3, "working")], 7)).toBe("done");
    expect(lane(item({ status: "dropped" }), [], 7)).toBe("done");
  });
});

describe("takenOnly", () => {
  it("is true while only sessions that took it are live, so the user may launch over them", () => {
    const i = item({ sessions: [3, 4], taken: [4] });
    expect(takenOnly(i, [session(4, "working")], 7)).toBe(true);
    expect(takenOnly(i, [session(3, "idle"), session(4, "working")], 7)).toBe(false);
    expect(takenOnly(i, [], 7)).toBe(false);
    expect(takenOnly(i, [session(4, "working")], 8)).toBe(false);
    expect(takenOnly({ ...i, status: "proposed" }, [session(4, "working")], 7)).toBe(false);
  });
});

describe("cardActions", () => {
  it("offers Launch beside Open only while takes alone are live", () => {
    const i = item({ project: "/r", sessions: [3, 4], taken: [4] });
    expect(cardActions(i, [session(4, "working")], 7)).toEqual(["open", "launch"]);
    expect(cardActions(i, [session(3, "idle"), session(4, "working")], 7)).toEqual(["open"]);
    expect(cardActions(i, [], 7)).toEqual(["launch"]);
    expect(cardActions({ ...i, project: undefined }, [], 7)).toEqual(["folder"]);
  });

  it("offers Accept for an agent's own filed work, also while its session runs", () => {
    const filed = item({ project: "/r", status: "proposed", sessions: [4], taken: [4], filed_by: [7, 4] });
    expect(cardActions(filed, [session(4, "working")], 7)).toEqual(["accept", "open"]);
    expect(cardActions(filed, [], 7)).toEqual(["accept"]);
  });
});

describe("byLane", () => {
  it("sorts each lane in the order to work it, and done newest first", () => {
    const lanes = byLane([item({ id: 1, updated: 5 }), item({ id: 2, updated: 9 }), item({ id: 3, status: "shipped" })], [], 7);
    expect(lanes.backlog.map((i) => i.id)).toEqual([1, 2]);
    expect(lanes.done.map((i) => i.id)).toEqual([3]);
    const done = byLane([item({ id: 4, status: "shipped", updated: 1 }), item({ id: 5, status: "dropped", updated: 9 })], [], 7);
    expect(done.done.map((i) => i.id)).toEqual([5, 4]);
  });

  it("puts ready items first, then by priority, then the oldest", () => {
    const all = [
      item({ id: 1 }),
      item({ id: 2, priority: "p0", blocked_by: [5] }), // #5 still open
      item({ id: 3, priority: "p1", blocked_by: [6] }), // #6 shipped
      item({ id: 4, priority: "p1" }),
      item({ id: 5, priority: "p3" }),
      item({ id: 6, status: "shipped" }),
      item({ id: 7, blocked_by: [99] }), // #99 deleted
    ];
    expect(byLane(all, [], 7).backlog.map((i) => i.id)).toEqual([3, 4, 1, 7, 5, 2]);
    expect(waitingOn(all[1], all)).toEqual([5]);
    // What it waits on may be in a repository the board doesn't show.
    expect(byLane([all[1], all[3]], [], 7, all).backlog.map((i) => i.id)).toEqual([4, 2]);
    expect(byLane([all[1], all[3]], [], 7).backlog.map((i) => i.id)).toEqual([2, 4]);
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

  it("sends priority and what it comes after only when changed here", () => {
    expect(editChange(was, was, { ...mine, priority: "p2", blocked_by: [] })).toBeNull();
    expect(editChange(was, was, { ...mine, priority: "p0", blocked_by: [3, 2, 3] })).toEqual({
      do: "edit",
      priority: "p0",
      blocked_by: [3, 2, 3],
    });
    const now = { ...was, priority: "p1" as const };
    expect(editChange(was, now, { ...mine, priority: "p3" })).toMatchObject({ clash: expect.stringMatching(/priority/) });
    expect(editChange(was, now, { ...mine, priority: "p2" })).toBeNull();
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

  it("marks what an agent filed and the user hasn't accepted", () => {
    const shipped = [{ id: 6, title: "", branch: "fix/6", branch_taken: true, filed: true, against: "origin/main" }];
    expect(wouldShip(shipped, [])).toBe(
      "Tap again to ship #6 fix/6 (agent-filed, not yet accepted), merged into origin/main (as of the last fetch)",
    );
  });
});

describe("repo filter", () => {
  const a = item({ id: 1, project: "/home/u/valkyrie" });
  const b = item({ id: 2, project: "/home/u/notes" });
  const none = item({ id: 3 });

  it("shows every item unfiltered, one repository's, or those with no folder", () => {
    expect([a, b, none].filter((i) => inRepo(i, null)).map((i) => i.id)).toEqual([1, 2, 3]);
    expect([a, b, none].filter((i) => inRepo(i, "/home/u/notes")).map((i) => i.id)).toEqual([2]);
    expect([a, b, none].filter((i) => inRepo(i, "")).map((i) => i.id)).toEqual([3]);
  });

  it("offers each repository once by name, then no folder", () => {
    expect(repoChoices([a, b, none, item({ id: 4, project: "/home/u/notes" })])).toEqual([
      { repo: "/home/u/notes", label: "notes" },
      { repo: "/home/u/valkyrie", label: "valkyrie" },
      { repo: "", label: "No folder" },
    ]);
  });

  it("names repositories that share a name by their path", () => {
    const other = item({ id: 5, project: "/work/valkyrie" });
    expect(repoChoices([a, other]).map((c) => c.label)).toEqual(["/home/u/valkyrie", "/work/valkyrie"]);
  });
});

describe("launch offer", () => {
  const offer = (project: string, model?: string): LaunchOffer => ({ id: 1, project, item: 3, title: "t", agent: "claude", model, why: "w", made: 0 });

  it("shows the shown repository's, and what it starts on", () => {
    expect(launchesShown([offer("/a"), offer("/b")], "/b").map((o) => o.project)).toEqual(["/b"]);
    expect(launchesShown([offer("/a"), offer("/b")], null)).toHaveLength(2);
    expect(launchesOn(offer("/a"))).toBe("claude");
    expect(launchesOn(offer("/a", "opus"))).toBe("claude · opus");
  });
});

describe("build order", () => {
  const order = (project: string): OrderProposal => ({ id: 1, project, made: 0, trigger: "you asked", steps: [] });

  it("is asked for the shown repository, else the only one with open items", () => {
    const items = [item({ id: 1, project: "/a" }), item({ id: 2, project: "/a" }), item({ id: 3, project: "/b", status: "shipped" })];
    expect(planScope("/b", items)).toBe("/b");
    expect(planScope("", items)).toBeNull();
    expect(planScope(null, items)).toBe("/a");
    expect(planScope(null, [...items, item({ id: 4, project: "/c" })])).toBeNull();
    expect(ordersShown([order("/a"), order("/b")], "/b").map((o) => o.project)).toEqual(["/b"]);
    expect(ordersShown([order("/a"), order("/b")], null)).toHaveLength(2);
  });

  it("says what each step changes and what accepting did", () => {
    const it1 = item({ id: 1, priority: "p2", blocked_by: [5] });
    expect(stepChanges({ item: 1, reason: "", priority: "p1", blocked_by: [2, 5] }, it1)).toBe("p2→p1, after #2");
    expect(stepChanges({ item: 1, reason: "" }, it1)).toBe("");
    expect(changesSomething({ item: 1, reason: "" })).toBe(false);
    expect(changesSomething({ item: 1, reason: "", blocked_by: [] })).toBe(true);
    expect(appliedSummary(true, { changed: [3, 2], skipped: [[1, "edited since"]] })).toBe("Changed #3 #2; #1 left: edited since");
    expect(appliedSummary(false, { changed: [] })).toBe("Build order declined: nothing changed");
  });
});

describe("startAgent", () => {
  it("starts an item on the agent it was last launched with, before this device's pick", () => {
    expect(startAgent(item({ launch: { agent: "codex", model: "gpt-5.5" } }), "claude")).toBe("codex");
    expect(startAgent(item({ launch: { agent: "claude" } }), "codex")).toBe("claude");
  });
  it("falls back to the agent last picked here, then claude", () => {
    expect(startAgent(item(), "codex")).toBe("codex");
    expect(startAgent(null, null)).toBe("claude");
    expect(startAgent(item({ launch: { agent: "aider" } }), "nonsense")).toBe("claude");
  });
});
