import { describe, expect, it } from "vitest";
import { answeringLabel, choicesKey, type Answering } from "./answering";
import type { Choice } from "./choices";

const yesNo: Choice[] = [
  { label: "Yes", selected: true, n: 1 },
  { label: "No", selected: false, n: 2 },
];

describe("answeringLabel", () => {
  it("is null with no answer under way", () => {
    expect(answeringLabel(undefined, yesNo)).toBeNull();
  });

  it("gives the tapped label for the prompt answered, sent or confirmed", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: false };
    expect(answeringLabel(a, yesNo)).toBe("Yes");
    expect(answeringLabel({ ...a, done: true }, yesNo)).toBe("Yes");
  });

  it("goes live again when the screen shows another prompt", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: true };
    const next: Choice[] = [
      { label: "Allow", selected: true, n: 1 },
      { label: "Deny", selected: false, n: 2 },
    ];
    expect(answeringLabel(a, next)).toBeNull();
  });

  it("ignores which choice the cursor is on", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "No", done: false };
    const moved = yesNo.map((c) => ({ ...c, selected: !c.selected }));
    expect(answeringLabel(a, moved)).toBe("No");
  });
});
