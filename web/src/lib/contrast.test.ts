// Every look's text colors clear WCAG AA (4.5:1) on the surfaces they're drawn on (#188).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
// Read from source: theme.ts applies the saved look to the page as it loads.
const LOOKS = [
  .../export const LOOKS: Look\[\] = \[([^\]]*)\]/.exec(readFileSync(new URL("./theme.ts", import.meta.url), "utf8"))![1].matchAll(/"(\w+)"/g),
].map((m) => m[1]);

/** Each look's own hex tokens; the default look is the bare `:root` block that sets `--page`. */
function palettes(): Record<string, Record<string, string>> {
  const out: Record<string, Record<string, string>> = {};
  for (const m of css.matchAll(/:root(?:\[data-theme="(\w+)"\])?\s*\{([^}]*)\}/g)) {
    if (!/--page:/.test(m[2])) continue;
    const vars: Record<string, string> = {};
    for (const v of m[2].matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})\s*;/gi)) vars[v[1]] = v[2];
    out[m[1] ?? "default"] = vars;
  }
  const base = out.default;
  for (const k of Object.keys(out)) if (k !== "default") out[k] = { ...base, ...out[k] };
  return out;
}

const luminance = (hex: string) => {
  const [r, g, b] = [1, 3, 5]
    .map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a: string, b: string) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

/** Body text, on anything including a raised chip, toast or code span. */
const TEXT = ["fg", "soft", "muted"];
/** State and accent colors, as labels on a page, panel or card (and the page's color on them, as pills). */
const STATUS = ["idle", "accent", "accent2", "needs", "blocked", "done", "working", "interrupted"];
const AA = 4.5;

describe("theme contrast", () => {
  const all = palettes();

  it("has a palette for every look", () => {
    expect(Object.keys(all).sort()).toEqual([...LOOKS].sort());
  });

  for (const [look, t] of Object.entries(all)) {
    it(`${look}: text clears AA`, () => {
      const fails: string[] = [];
      const check = (fg: string, bgs: string[]) => {
        for (const bg of bgs) {
          const r = ratio(t[fg], t[bg]);
          if (r < AA) fails.push(`--${fg} ${t[fg]} on --${bg} ${t[bg]}: ${r.toFixed(2)}`);
        }
      };
      for (const k of TEXT) check(k, ["page", "surface", "card", "raised"]);
      for (const k of STATUS) check(k, ["page", "surface", "card"]);
      check("on-accent", ["accent"]);
      expect(fails).toEqual([]);
    });
  }
});
