import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** Every rule's selector, with the at-rules it sits inside. */
function rules(): { sel: string; at: string[] }[] {
  const out: { sel: string; at: string[] }[] = [];
  const stack: string[] = [];
  let head = "";
  for (const ch of css) {
    if (ch === "{") {
      const h = head.trim();
      if (h.startsWith("@")) stack.push(h);
      else {
        out.push({ sel: h, at: [...stack] });
        stack.push("");
      }
      head = "";
    } else if (ch === "}") {
      stack.pop();
      head = "";
    } else if (ch === ";") {
      head = "";
    } else head += ch;
  }
  return out;
}

/** The declarations of the first rule whose selector is exactly `sel`. */
function rule(sel: string): string | undefined {
  const esc = sel.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return css.match(new RegExp(`(?:^|\\})\\s*${esc}\\s*\\{([^}]*)\\}`, "m"))?.[1];
}

describe("hover and focus (#199)", () => {
  // A phone fires :hover on tap and keeps it until the next tap elsewhere, so a
  // tapped row or button stayed lit. Hover styles are for pointers that hover.
  it("keeps every :hover rule inside @media (hover: hover)", () => {
    const loose = rules()
      .filter((r) => r.sel.includes(":hover"))
      .filter((r) => !r.at.some((a) => /^@media[^{]*\(hover:\s*hover\)/.test(a)))
      .map((r) => r.sel);
    expect(loose).toEqual([]);
  });

  it("draws keyboard focus with the shared ring", () => {
    expect(rule(":focus-visible")).toMatch(/outline:\s*var\(--focus-ring\)/);
  });

  // The j/k pick is a cursor, not focus, but it should look like the same idea.
  it("draws the j/k pick with the same ring as focus", () => {
    const picked = rule(".card-slot.picked > .card,\n.row.picked,\n:is(.row, .card):focus-visible");
    expect(picked).toMatch(/outline:\s*var\(--focus-ring\)/);
  });

  it("spells the ring once, in the token", () => {
    expect(css.match(/outline:\s*2px solid/g) ?? []).toEqual([]);
    expect(css).toMatch(/--focus-ring:\s*2px solid var\(--focus-ring-color\)/);
  });
});
