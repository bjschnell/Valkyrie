import { describe, expect, it } from "vitest";
import { chord, chordTarget } from "./chords";
import type { KeyLike } from "./termkeys";

function key(k: string, code: string, ...mods: ("ctrl" | "alt" | "shift" | "meta" | "altgr")[]): KeyLike {
  return {
    key: k,
    code,
    ctrlKey: mods.includes("ctrl"),
    altKey: mods.includes("alt"),
    shiftKey: mods.includes("shift"),
    metaKey: mods.includes("meta"),
    altGraph: mods.includes("altgr"),
  };
}

describe("chord", () => {
  it("reads the TUI's Alt chords", () => {
    expect(chord(key("j", "KeyJ", "alt"))).toEqual({ go: "next" });
    expect(chord(key("k", "KeyK", "alt"))).toEqual({ go: "prev" });
    expect(chord(key("n", "KeyN", "alt"))).toEqual({ go: "needs" });
    expect(chord(key("h", "KeyH", "alt"))).toEqual({ go: "home" });
    expect(chord(key("p", "KeyP", "alt"))).toEqual({ go: "last" });
    expect(chord(key("3", "Digit3", "alt"))).toEqual({ go: "tab", n: 3 });
  });

  it("leaves every other key alone", () => {
    expect(chord(key("j", "KeyJ"))).toBeNull();
    expect(chord(key("b", "KeyB", "alt"))).toBeNull();
    expect(chord(key("0", "Digit0", "alt"))).toBeNull();
    expect(chord(key("j", "KeyJ", "alt", "ctrl"))).toBeNull();
    expect(chord(key("J", "KeyJ", "alt", "shift"))).toBeNull();
    expect(chord(key("j", "KeyJ", "alt", "meta"))).toBeNull();
    expect(chord(key("ArrowDown", "ArrowDown", "alt"))).toBeNull();
  });

  it("takes the letter from the key on a Latin layout, the position otherwise", () => {
    // Dvorak: J sits where QWERTY has C.
    expect(chord(key("j", "KeyC", "alt"))).toEqual({ go: "next" });
    // A Mac's Option+J types ∆; a Cyrillic layout has о on that key.
    expect(chord(key("∆", "KeyJ", "alt"), true)).toEqual({ go: "next" });
    expect(chord(key("о", "KeyJ", "alt"))).toEqual({ go: "next" });
  });

  it("types what Option or AltGr types", () => {
    // A German Mac: Option+N is ~, Option+5 is [.
    expect(chord(key("~", "KeyN", "alt"), true)).toBeNull();
    expect(chord(key("[", "Digit5", "alt"), true)).toBeNull();
    expect(chord(key("@", "KeyQ", "ctrl", "alt", "altgr"))).toBeNull();
  });
});

describe("chordTarget", () => {
  const at = (open: number | null, last: number | null = null, needs: number[] = []) => ({
    sessions: [4, 7, 9],
    needs,
    open,
    last,
  });

  it("steps through the sessions and wraps", () => {
    expect(chordTarget({ go: "next" }, at(4))).toEqual({ session: 7 });
    expect(chordTarget({ go: "next" }, at(9))).toEqual({ session: 4 });
    expect(chordTarget({ go: "prev" }, at(4))).toEqual({ session: 9 });
    expect(chordTarget({ go: "next" }, at(null))).toEqual({ session: 4 });
    expect(chordTarget({ go: "prev" }, at(null))).toEqual({ session: 9 });
  });

  it("jumps to a session by its number in the list", () => {
    expect(chordTarget({ go: "tab", n: 2 }, at(4))).toEqual({ session: 7 });
    expect(chordTarget({ go: "tab", n: 5 }, at(4))).toBeUndefined();
    expect(chordTarget({ go: "tab", n: 1 }, at(4))).toBeUndefined();
  });

  it("goes to whoever needs you next", () => {
    expect(chordTarget({ go: "needs" }, at(4, null, [9, 7]))).toEqual({ session: 9 });
    expect(chordTarget({ go: "needs" }, at(9, null, [9, 7]))).toEqual({ session: 7 });
    expect(chordTarget({ go: "needs" }, at(7, null, [7]))).toBeUndefined();
    expect(chordTarget({ go: "needs" }, at(4))).toBeUndefined();
  });

  it("goes home and back to the last session", () => {
    expect(chordTarget({ go: "home" }, at(4))).toBe("home");
    expect(chordTarget({ go: "home" }, at(null))).toBeUndefined();
    expect(chordTarget({ go: "last" }, at(4, 9))).toEqual({ session: 9 });
    expect(chordTarget({ go: "last" }, at(4, 12))).toBeUndefined();
    expect(chordTarget({ go: "last" }, at(4))).toBeUndefined();
  });
});
