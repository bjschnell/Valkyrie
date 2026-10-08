import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { findChoices, plainYes, promptContext } from "./choices";

// Real screens recorded from Claude Code and Codex (crates/valkyrie-agents).
const fixture = (name: string) =>
  readFileSync(new URL(`../../../crates/valkyrie-agents/tests/fixtures/${name}`, import.meta.url), "utf8")
    .split("\n");

describe("findChoices", () => {
  it("reads Claude's permission dialog, wrapped options included", () => {
    const choices = findChoices(fixture("claude_permission_bash.txt"));
    expect(choices.map((c) => c.n)).toEqual([1, 2, 3, 4]);
    expect(choices[0]).toMatchObject({ label: "Yes", selected: true });
    expect(choices[1].label).toBe("Yes, and always allow access to /home/user/project from this project");
    expect(choices[3]).toMatchObject({ label: "No", selected: false });
  });

  it("reads Codex's approval, without the key hints", () => {
    const choices = findChoices(fixture("codex_approval.txt"));
    expect(choices.map((c) => c.label)).toEqual([
      "Yes, proceed",
      "Yes, and don't ask again for commands that start with `touch b.txt`",
      "No, and tell Codex what to do differently",
    ]);
    expect(choices[0].selected).toBe(true);
  });

  it("reads Claude's unnumbered trust check as cursor moves", () => {
    const choices = findChoices(fixture("claude_trust.txt"));
    expect(choices).toEqual([
      { label: "No, exit", selected: true, moves: 0 },
      { label: "Yes, I trust this folder", selected: false, moves: 1 },
    ]);
  });

  it("finds nothing on an idle or working screen", () => {
    expect(findChoices(fixture("claude_idle_live.txt"))).toEqual([]);
    expect(findChoices(fixture("claude_busy_live.txt"))).toEqual([]);
    expect(findChoices(fixture("codex_idle.txt"))).toEqual([]);
  });

  it("ignores a numbered list with no cursor on it", () => {
    expect(findChoices(["Plan:", "1. build", "2. test", "3. ship", "$ "])).toEqual([]);
  });

  it("ignores a numbered list in ordinary output far above", () => {
    const lines = ["Steps:", "1. build", "2. test", ...Array(40).fill("log line"), "$ "];
    expect(findChoices(lines)).toEqual([]);
  });
});

describe("plainYes", () => {
  const c = (...labels: string[]) => labels.map((label, i) => ({ label, selected: i === 0, n: i + 1 }));
  it("picks a first choice that is a plain yes", () => {
    expect(plainYes(c("Yes", "Yes, and don't ask again", "No"))?.label).toBe("Yes");
    expect(plainYes(c("Yes, proceed", "No, and tell Codex what to do differently"))?.label).toBe("Yes, proceed");
  });
  it("never picks a broader grant or a refusal", () => {
    expect(plainYes(c("Yes, allow all edits during this session", "No"))).toBeNull();
    expect(plainYes(c("Always allow", "No"))).toBeNull();
    expect(plainYes(c("No", "Yes"))).toBeNull();
    expect(plainYes([])).toBeNull();
  });
});

describe("promptContext", () => {
  it("says what Claude's permission prompt is about", () => {
    expect(promptContext(fixture("claude_permission_bash.txt"))).toBe("touch acc.txt — Create empty acc.txt file");
  });
  it("says what Codex's approval is about", () => {
    expect(promptContext(fixture("codex_approval.txt"))).toBe("touch b.txt");
  });
  it("is nothing without a prompt", () => {
    expect(promptContext(["$ ls", "a b c"])).toBeNull();
  });
});
