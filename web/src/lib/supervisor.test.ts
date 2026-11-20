import { describe, expect, it } from "vitest";
import { watchLine } from "./supervisor";

describe("watchLine", () => {
  it("says how many, when read and when next, or why not", () => {
    expect(watchLine({ prs: [1, 2], polled_at: 880, next_at: 1180, on: true }, 1_000_000)).toBe("2 PRs · read 2m ago · next in 3m");
    expect(watchLine({ prs: [1], polled_at: 880, next_at: 900, on: true }, 1_000_000)).toBe("1 PR · read 2m ago");
    expect(watchLine({ prs: [], on: true }, 1_000_000)).toBe("the poll hasn't run yet");
    expect(watchLine({ prs: [], polled_at: 1, on: false }, 1_000_000)).toBe("off: nothing is read");
  });
});
