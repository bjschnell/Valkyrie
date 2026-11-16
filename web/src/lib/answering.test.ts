import { describe, expect, it } from "vitest";
import { answeringLabel, choicesKey, type Answering } from "./answering";
import type { Choice } from "./choices";

const yesNo: Choice[] = [
  { label: "Yes", selected: true, n: 1 },
  { label: "No", selected: false, n: 2 },
];

describe("answeringLabel", () => {
  it("is null with no answer under way", () => {
    expect(answeringLabel(undefined, yesNo, 7)).toBeNull();
  });

  it("gives the tapped label for the prompt answered, sent or confirmed", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: false, seq: 7 };
    expect(answeringLabel(a, yesNo, 7)).toBe("Yes");
    expect(answeringLabel({ ...a, done: true }, yesNo, 7)).toBe("Yes");
  });

  it("goes live again when the screen shows another prompt", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: true, seq: 7 };
    const next: Choice[] = [
      { label: "Allow", selected: true, n: 1 },
      { label: "Deny", selected: false, n: 2 },
    ];
    expect(answeringLabel(a, next, 7)).toBeNull();
  });

  it("ignores which choice the cursor is on", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "No", done: false, seq: 7 };
    const moved = yesNo.map((c) => ({ ...c, selected: !c.selected }));
    expect(answeringLabel(a, moved, 7)).toBe("No");
  });

  it("goes live for the next prompt with the same choices once confirmed", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: true, seq: 7 };
    expect(answeringLabel(a, yesNo, 9)).toBeNull();
  });

  it("stays dim in flight even if the status moves before the confirm", () => {
    const a: Answering = { of: choicesKey(yesNo), label: "Yes", done: false, seq: 7 };
    expect(answeringLabel(a, yesNo, 8)).toBe("Yes");
  });
});
