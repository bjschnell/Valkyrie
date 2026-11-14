import { describe, expect, it } from "vitest";
import { stepChoices } from "./step";
import type { Flag } from "../proto";

const flag = (step?: Flag["step"]): Flag => ({
  reason: "CI red on PR #98: windows",
  pr: "u",
  seq: 3,
  step,
});

describe("stepChoices", () => {
  it("offers the step, its alternative, then leave", () => {
    expect(
      stepChoices(flag({ action: "rerun", why: "matches #17", alt: "ci-fix" })),
    ).toEqual([
      { action: "rerun", label: "Rerun failed jobs", primary: true },
      { action: "ci-fix", label: "Ask its agent to fix CI", primary: false },
      { action: "leave", label: "Leave it", primary: false },
    ]);
    expect(
      stepChoices(flag({ action: "merge-base", why: "conflicts" })).map(
        (c) => c.action,
      ),
    ).toEqual(["merge-base", "leave"]);
  });

  it("offers nothing until a step is picked", () => {
    expect(stepChoices(flag())).toEqual([]);
    expect(stepChoices(undefined)).toEqual([]);
  });
});
