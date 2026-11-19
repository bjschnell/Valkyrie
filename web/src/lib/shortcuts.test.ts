import { describe, expect, it } from "vitest";
import { entries, paletteKey } from "./palette";
import { sections, shortcutsKey, type PageKey } from "./shortcuts";
import type { KeyLike } from "./termkeys";

const press = (key: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  key,
  code: "",
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  altGraph: false,
  ...mods,
});

/** An element that is, or sits inside, one of the `tags` (no DOM here, so a stand-in for closest). */
const field = (tag: string) => ({ closest: (sel: string) => (sel.split(/,\s*/).some((t) => t.startsWith(tag)) ? {} : null) }) as unknown as Element;

describe("shortcutsKey", () => {
  it("is a bare ? when nothing is being typed", () => {
    expect(shortcutsKey(press("?", { shiftKey: true }), null)).toBe(true);
    expect(shortcutsKey(press("?"), field("div"))).toBe(true);
  });
  it("leaves ? to a text field, a select and the terminal's input", () => {
    for (const html of ["input", "textarea", "select", "[contenteditable]"]) {
      expect(shortcutsKey(press("?"), field(html))).toBe(false);
    }
  });
  it("ignores other keys and chords", () => {
    expect(shortcutsKey(press("/"), null)).toBe(false);
    expect(shortcutsKey(press("?", { ctrlKey: true }), null)).toBe(false);
    expect(shortcutsKey(press("?", { altKey: true }), null)).toBe(false);
    expect(shortcutsKey(press("?", { metaKey: true }), null)).toBe(false);
  });
});

describe("sections", () => {
  const pages: PageKey[] = [
    "inbox", "new", "settings", "backlog", "decisions", "docs", "digest", "past",
    "wiki", "search", "supervisor", "doc", "item", "order", "session", "review",
  ];
  it("puts the page's keys first and the global ones last, for every page", () => {
    for (const p of pages) {
      const s = sections(p, false);
      expect(s.length).toBeGreaterThanOrEqual(2);
      expect(s.at(-1)?.title).toBe("Everywhere");
      expect(s[0].shortcuts.length).toBeGreaterThan(0);
    }
  });
  it("names Option and ⌘ on a Mac", () => {
    const flat = (mac: boolean) => JSON.stringify(sections("inbox", mac));
    expect(flat(true)).toContain("Option");
    expect(flat(true)).toContain("⌘");
    expect(flat(false)).toContain("Alt");
    expect(flat(false)).not.toContain("Option");
  });
  it("has no key twice in a section", () => {
    for (const p of pages) {
      for (const s of sections(p, false)) {
        const caps = s.shortcuts.flatMap((x) => x.keys.map((k) => k.join("+")));
        expect(new Set(caps).size).toBe(caps.length);
      }
    }
  });
  it("lists the palette's key, the one paletteKey accepts", () => {
    expect(paletteKey(press("k", { ctrlKey: true, code: "KeyK" }), false, false)).toBe(true);
    const everywhere = sections("inbox", false).at(-1)!;
    expect(everywhere.shortcuts.some((x) => x.keys.some((k) => k.join("+") === "Ctrl+K"))).toBe(true);
  });
});

describe("palette", () => {
  it("offers the sheet as an action, so a terminal can reach it", () => {
    const list = entries({
      sessions: [], needs: [], proposals: [], items: [], orders: [], docs: [], pages: [], looks: [], look: "",
    });
    expect(list.find((e) => e.key === "shortcuts")?.do).toEqual({ sheet: true });
  });
});
