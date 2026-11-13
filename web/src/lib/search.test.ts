import { describe, expect, it } from "vitest";
import { marks, terms, windowStart } from "./search";

describe("session search", () => {
  it("reads the query as the daemon does", () => {
    expect(terms('Zoom  "One   Key" zoom')).toEqual(["zoom", "one key"]);
    expect(terms('  "" ')).toEqual([]);
  });

  it("marks every place a word appears, in any case", () => {
    expect(marks("Zoom the pane, zoom out", ["zoom", "pane"])).toEqual([
      { text: "Zoom", hit: true },
      { text: " the ", hit: false },
      { text: "pane", hit: true },
      { text: ", ", hit: false },
      { text: "zoom", hit: true },
      { text: " out", hit: false },
    ]);
    expect(marks("abc", [])).toEqual([{ text: "abc", hit: false }]);
    // Overlapping words merge into one marked run.
    expect(marks("hooked", ["hook", "oked"])).toEqual([{ text: "hooked", hit: true }]);
  });

  it("leaves the furthest window to the daemon", () => {
    expect(windowStart("3d", 10 * 86_400_000)).toBe(7 * 86_400_000);
    expect(windowStart("all", 1)).toBeNull();
  });
});
