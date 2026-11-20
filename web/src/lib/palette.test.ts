import { describe, expect, it } from "vitest";
import type { Decision, Doc, Item, SessionInfo } from "../proto";
import { entries, paletteKey, rank, type Sources } from "./palette";
import type { KeyLike } from "./termkeys";

function key(k: string, code: string, ...mods: ("ctrl" | "alt" | "shift" | "meta")[]): KeyLike {
  return {
    key: k,
    code,
    ctrlKey: mods.includes("ctrl"),
    altKey: mods.includes("alt"),
    shiftKey: mods.includes("shift"),
    metaKey: mods.includes("meta"),
    altGraph: false,
  };
}

describe("paletteKey", () => {
  it("opens on Ctrl+K off a Mac, but leaves it to a terminal that has the keyboard", () => {
    expect(paletteKey(key("k", "KeyK", "ctrl"), false, false)).toBe(true);
    expect(paletteKey(key("k", "KeyK", "ctrl"), false, true)).toBe(false);
  });
  it("opens on Ctrl+Shift+K anywhere, terminal included", () => {
    expect(paletteKey(key("K", "KeyK", "ctrl", "shift"), false, true)).toBe(true);
    expect(paletteKey(key("K", "KeyK", "ctrl", "shift"), true, true)).toBe(true);
  });
  it("opens on Cmd+K on a Mac, where Ctrl+K stays the field's", () => {
    expect(paletteKey(key("k", "KeyK", "meta"), true, true)).toBe(true);
    expect(paletteKey(key("k", "KeyK", "ctrl"), true, false)).toBe(false);
    expect(paletteKey(key("k", "KeyK", "meta"), false, false)).toBe(false);
  });
  it("goes by the letter, not where it sits, and ignores Alt+K", () => {
    // Dvorak's K is where QWERTY has V.
    expect(paletteKey(key("k", "KeyV", "ctrl"), false, false)).toBe(true);
    expect(paletteKey(key("v", "KeyK", "ctrl"), false, false)).toBe(false);
    expect(paletteKey(key("k", "KeyK", "alt"), false, false)).toBe(false);
    expect(paletteKey(key("k", "KeyK", "ctrl", "alt"), false, false)).toBe(false);
  });
  it("reads a non-Latin layout's K by position", () => {
    expect(paletteKey(key("л", "KeyK", "ctrl"), false, false)).toBe(true);
  });
});

const session = (id: number, name: string): SessionInfo =>
  ({ id, name, cwd: `/home/u/repos/${name}`, command: [], pid: 1, created_unix: 0, title: null, clients: 0, exited: null, status: { state: "working" } }) as unknown as SessionInfo;

const sources = (over: Partial<Sources> = {}): Sources => ({
  sessions: [session(1, "valkyrie"), session(2, "homelab")],
  needs: [],
  proposals: [],
  items: [],
  orders: [],
  docs: [],
  pages: [
    { label: "Backlog", hash: "#/backlog" },
    { label: "Settings", hash: "#/settings" },
  ],
  looks: [
    { key: "default", name: "Dracula", about: "dark" },
    { key: "matrix", name: "Matrix", about: "green" },
  ],
  look: "default",
  ...over,
});

describe("entries", () => {
  it("puts actions first, then sessions, pages and themes", () => {
    const groups = entries(sources()).map((e) => e.group);
    expect(groups).toEqual(["Actions", "Actions", "Actions", "Sessions", "Sessions", "Pages", "Pages", "Themes", "Themes"]);
  });
  it("offers to accept proposed decisions and backlog items, and no others", () => {
    const d = (id: number, status: string) =>
      ({ id, project: "/r/valkyrie", title: `d${id}`, kind: "gotcha", status, provenance: { by: "claude" } }) as unknown as Decision;
    const i = (id: number, status: string) => ({ id, number: id, title: `i${id}`, kind: "bug", status, by: "claude" }) as unknown as Item;
    const list = entries(sources({ proposals: [d(1, "proposed"), d(2, "active")], items: [i(3, "proposed"), i(4, "open"), i(5, "shipped")] }));
    const accepts = list.filter((e) => e.label.startsWith("Accept"));
    expect(accepts.map((e) => e.label)).toEqual(["Accept gotcha: d1", "Accept #3: i3"]);
    expect(accepts[1].do).toEqual({ item: 3 });
    // Enter straight away goes somewhere; it never accepts.
    expect(list[0].label).toBe("New session");
    // Open and proposed items can be jumped to; shipped ones can't.
    expect(list.filter((e) => e.group === "Backlog").map((e) => e.do)).toEqual([{ go: "#/backlog/3" }, { go: "#/backlog/4" }]);
  });
  it("leads with who needs you, when someone does", () => {
    const first = entries(sources({ needs: [2] }))[0];
    expect(first).toMatchObject({ label: "Go to who needs you", sub: "homelab", do: { go: "#/s/2" } });
  });
  it("links docs by key and skips those without one", () => {
    const docs = [{ title: "Plan", kind: "plan", key: "p1a", path: "x.md" }, { title: "Old", kind: "note", path: "y.md" }] as Doc[];
    const list = entries(sources({ docs })).filter((e) => e.group === "Docs");
    expect(list.map((e) => e.do)).toEqual([{ go: "#/memory/docs/p1a" }]);
  });
  it("marks the look in use", () => {
    const looks = entries(sources({ look: "matrix" })).filter((e) => e.group === "Themes");
    expect(looks.map((e) => e.sub)).toEqual(["dark", "green · current"]);
  });
});

describe("rank", () => {
  const list = entries(sources());
  it("keeps the order for an empty query", () => {
    expect(rank(list, "  ")).toBe(list);
  });
  it("finds by a word's start ahead of letters scattered", () => {
    expect(rank(list, "home")[0].label).toBe("homelab");
    expect(rank(list, "set")[0].label).toBe("Settings");
  });
  it("finds by scattered letters in order", () => {
    expect(rank(list, "vlkr").map((e) => e.label)).toContain("valkyrie");
    expect(rank(list, "zzz")).toEqual([]);
  });
  it("needs every word to match", () => {
    expect(rank(list, "theme matrix").map((e) => e.label)).toEqual(["Theme: Matrix"]);
  });
  it("finds by the hidden words too", () => {
    expect(rank(list, "launch")[0].label).toBe("New session");
  });
});
