import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
const tsx = readFileSync(new URL("../components/Backlog.tsx", import.meta.url), "utf8");

/** The flex-grow a `.item-dock <sel>` rule gives a button; a bare `flex: none` is 0. */
function grow(sel: string): number {
  const rule = css.match(new RegExp(`\\.item-dock ${sel} \\{([^}]*)\\}`));
  const flex = rule?.[1].match(/flex:\s*([^;]+);/)?.[1].trim();
  if (flex === undefined) return NaN;
  return flex === "none" ? 0 : parseFloat(flex);
}

describe("new backlog item dock (#101)", () => {
  const footer = tsx.match(/<footer className="dock item-dock">([\s\S]*?)<\/footer>/)?.[1] ?? "";
  const buttons = (footer.match(/<button[\s\S]*?<\/button>/g) ?? []).map((block) => ({
    cls: block.match(/className="([^"]*)"/)?.[1] ?? "",
    // Text is whatever follows the opening tag's last `>` (handlers hold `=>`, so not the first).
    text: block.slice(block.slice(0, -"</button>".length).lastIndexOf(">") + 1, -"</button>".length).trim(),
  }));
  const save = buttons.find((b) => b.text === "Save");
  const launch = buttons.find((b) => b.text.includes("Save & launch"));

  it("makes Save the primary button", () => {
    expect(save).toBeDefined();
    expect(launch).toBeDefined();
    expect(save?.cls).toMatch(/\bprimary\b/);
    expect(launch?.cls).not.toMatch(/\bprimary\b/);
  });

  // #197 reversed #101's 2:1 split: the two are equal, and both read as buttons.
  it("gives Save and Save & launch the same width (#197)", () => {
    const [s, l] = [grow("\\.save"), grow("\\.launch")];
    expect(s).toBeGreaterThan(0);
    expect(l).toBe(s);
  });

  it("draws Save & launch as a button, with a border and a fill (#197)", () => {
    const rule = css.match(/\.item-dock \.launch \{([^}]*)\}/)?.[1] ?? "";
    expect(rule).toMatch(/border:\s*1px solid/);
    expect(rule).toMatch(/background:\s*var\(--/);
    // The board card's `.launch` chip is an inline-flex, which pins the label left.
    expect(rule).toMatch(/justify-content:\s*center/);
  });
});
