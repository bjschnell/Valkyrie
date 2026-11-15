import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** The declarations of the first rule whose selector is exactly `sel`. */
function rule(sel: string): string | undefined {
  const esc = sel.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return css.match(new RegExp(`(?:^|\\})\\s*${esc}\\s*\\{([^}]*)\\}`, "m"))?.[1];
}

describe("the list beside a session on desktop (#195)", () => {
  // `.cards` was a grid with an auto track, so a card's unwrapped name sized the
  // track: on the 360px list the card ran past the column's edge, cutting off
  // its Open button and scrolling the list sideways.
  it("keeps cards to the column's width, whatever their names", () => {
    expect(rule(".cards")).toMatch(/grid-template-columns:\s*minmax\(0,\s*1fr\)/);
  });

  // Close sits under a session row for a swipe to uncover. A mouse can't swipe,
  // and a row's hover background is see-through, so Close showed through over
  // the row's text. It stays hidden until a swipe opens it (or one is under way).
  it("hides a session's Close button until the row is swiped", () => {
    const hidden = rule('.row-slot:not(:has(> .row.dragging)) > .row-close[aria-hidden="true"]');
    expect(hidden).toMatch(/visibility:\s*hidden/);
  });
});
