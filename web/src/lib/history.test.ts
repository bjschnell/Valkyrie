import { describe, expect, it } from "vitest";
import type { DigestEntry, PastSession, SupervisorEntry } from "../proto";
import { byDay, historyStart, shownCounts, shownOutcome, tileCounts, tileOf, timeline } from "./history";

const e = (key: string, last_ms: number, more: Partial<DigestEntry> = {}): DigestEntry => ({
  key,
  repo: "api",
  agent: "claude",
  what: `work ${key}`,
  outcome: "done",
  detail: "",
  prompts: 1,
  first_ms: last_ms - 1000,
  last_ms,
  ...more,
});
const past = (key: string): PastSession => ({ ...e(key, 0), started_ms: 0 });
const s = (at: number, said = "you took: fix CI"): SupervisorEntry => ({ at, project: "/r/api", said });
const never = () => false;

describe("shownOutcome", () => {
  it("splits a live session waiting on you out of stuck", () => {
    const asking = (id: number) => id === 4;
    expect(shownOutcome(e("a", 1, { outcome: "stuck", session: 4 }), asking)).toBe("asks");
    expect(shownOutcome(e("b", 1, { outcome: "stuck", session: 5 }), asking)).toBe("stuck");
    expect(shownOutcome(e("c", 1, { outcome: "stuck" }), asking)).toBe("stuck");
    expect(shownOutcome(e("d", 1, { outcome: "done", session: 4 }), asking)).toBe("done");
  });
});

describe("timeline", () => {
  it("merges sessions and the supervisor's steps newest first, each once", () => {
    const list = timeline({
      entries: [e("a", 5000), e("b", 2000, { session: 3 })],
      past: [past("a"), past("b"), past("z")],
      log: [s(4), s(1)],
      since: 1500,
      asking: never,
    });
    expect(list.map((t) => t.key)).toEqual(["sa", "v4:0", "sb"]);
    // An ended session can resume; a live one opens instead.
    expect(list[0].kind === "session" && list[0].past?.key).toBe("a");
    expect(list[2].kind === "session" && list[2].past).toBeUndefined();
  });

  it("counts the tiles from sessions only", () => {
    const list = timeline({
      entries: [e("a", 3, { outcome: "shipped" }), e("b", 2, { outcome: "stuck" }), e("c", 1), e("d", 1, { outcome: "waiting" })],
      past: [],
      log: [s(0)],
      since: 0,
      asking: never,
    });
    expect(tileCounts(list)).toEqual({ finished: 1, stuck: 1, shipped: 1 });
    expect(list.map(tileOf)).toEqual(["shipped", "stuck", "finished", null, null]);
  });
});

describe("byDay", () => {
  it("groups consecutive entries of a day", () => {
    const noon = new Date(2026, 10, 19, 12).getTime();
    const list = timeline({ entries: [e("a", noon), e("b", noon - 3600_000), e("c", noon - 86_400_000)], past: [], log: [], since: 0, asking: never });
    expect(byDay(list).map(([, ts]) => ts.length)).toEqual([2, 1]);
  });
});

describe("historyStart", () => {
  it("leaves since-you-left to the daemon and caps at a month", () => {
    const now = 100 * 86_400_000;
    expect(historyStart("away", now)).toBeNull();
    expect(historyStart("3d", now)).toBe(now - 3 * 86_400_000);
    expect(historyStart("month", now)).toBe(now - 31 * 86_400_000);
  });
});

describe("shownCounts", () => {
  it("counts a prompt as needs you, not stuck", () => {
    const list = timeline({
      entries: [e("a", 3, { outcome: "stuck", session: 1 }), e("b", 2, { outcome: "stuck" }), e("c", 1, { outcome: "shipped" })],
      past: [],
      log: [s(0)],
      since: 0,
      asking: (id) => id === 1,
    });
    expect(shownCounts(list)).toBe("1 needs you · 1 stuck · 1 shipped");
  });
});
