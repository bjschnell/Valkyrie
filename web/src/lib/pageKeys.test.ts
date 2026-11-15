import { describe, expect, it } from "vitest";
import { usePageKeys } from "./pageKeys";
import { PAGE_KEYS, sections } from "./shortcuts";

// The pages' keys and the '?' sheet are one table (#191): tsc, not the runtime,
// holds a page's bindings to it, so these only compile while that holds.
describe("usePageKeys's types", () => {
  it("rejects a key the table doesn't list and a listed key left unbound", () => {
    const never = () => {
      const noop = () => {};
      // @ts-expect-error z isn't one of the backlog page's keys
      usePageKeys("backlog", { n: noop, a: noop, q: noop, z: noop });
      // @ts-expect-error q is listed for the backlog page but not bound
      usePageKeys("backlog", { n: noop, a: noop });
      usePageKeys("backlog", { n: noop, a: noop, q: noop });
    };
    expect(typeof never).toBe("function");
  });
});

describe("the sheet from the table", () => {
  it("shows every key a page binds, and joins keys with the same words in one row", () => {
    for (const page of Object.keys(PAGE_KEYS) as (keyof typeof PAGE_KEYS)[]) {
      const shown = sections(page, false)[0].shortcuts.flatMap((x) => x.keys.map((k) => k.join("+")));
      for (const key of Object.keys(PAGE_KEYS[page])) {
        expect(shown).toContain(key === "Escape" ? "Esc" : key === "G" ? "Shift+G" : key);
      }
    }
    const inbox = sections("inbox", false)[0].shortcuts;
    expect(inbox.find((x) => x.does === "Open it")?.keys).toEqual([["Enter"], ["o"]]);
  });
});
