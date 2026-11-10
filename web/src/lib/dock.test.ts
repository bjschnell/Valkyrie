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

  it("gives Save more room than Save & launch, which takes about half as much", () => {
    const [s, l] = [grow("\\.save"), grow("\\.launch")];
    expect(s).toBeGreaterThan(0);
    expect(l).toBeGreaterThan(0);
    expect(l / s).toBeLessThanOrEqual(0.5);
  });
});
