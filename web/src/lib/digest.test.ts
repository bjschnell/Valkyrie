import { describe, expect, it } from "vitest";
import type { DigestEntry } from "../proto";
import { counts, groupDigest, outcomeLabel, spanOf, windowStart } from "./digest";

const at = (y: number, m: number, d: number, h: number, min = 0) => new Date(y, m - 1, d, h, min).getTime();

const entry = (key: string, repo: string, last: number, outcome: DigestEntry["outcome"] = "done"): DigestEntry => ({
  key,
  repo,
  agent: "claude",
  what: key,
  outcome,
  detail: "",
  prompts: 1,
  first_ms: last - 3_600_000,
  last_ms: last,
});

describe("away digest", () => {
  it("groups by repository in the daemon's order, or by day newest first", () => {
    const es = [
      entry("a", "valkyrie", at(2026, 11, 9, 12), "stuck"),
      entry("b", "dotfiles", at(2026, 11, 10, 12)),
      entry("c", "valkyrie", at(2026, 11, 10, 9)),
    ];
    expect(groupDigest(es, "repo").map(([g, list]) => [g, list.map((e) => e.key)])).toEqual([
      ["valkyrie", ["a", "c"]],
      ["dotfiles", ["b"]],
    ]);
    const days = groupDigest(es, "day").map(([, list]) => list.map((e) => e.key));
    expect(days).toEqual([["b", "c"], ["a"]]);
  });

  it("counts outcomes most urgent first and names the PR wait", () => {
    const es = [entry("a", "r", 1, "shipped"), entry("b", "r", 1, "waiting"), entry("c", "r", 1, "shipped")];
    expect(counts(es)).toBe("1 waiting on PR/CI · 2 shipped");
    expect(outcomeLabel("stuck")).toBe("stuck");
  });

  it("shows a span within a day as clock times, and across midnight with both days", () => {
    const same = entry("a", "r", at(2026, 11, 10, 11));
    expect(spanOf(same)).toMatch(/10.00.*11.00/);
    const across = { ...same, first_ms: at(2026, 11, 9, 23) };
    expect(spanOf(across)).toContain(" – ");
  });

  it("starts the window where it's picked", () => {
    const now = at(2026, 11, 10, 15);
    expect(windowStart("away", now)).toBeNull();
    expect(windowStart("12h", now)).toBe(now - 12 * 3_600_000);
    expect(windowStart("today", now)).toBe(at(2026, 11, 10, 0));
  });
});
