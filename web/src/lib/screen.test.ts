import { describe, expect, it } from "vitest";
import type { Row } from "../proto";
import { charWidth, rowRuns, textWidth } from "./screen";

const width = (ch: string) => charWidth(ch.codePointAt(0)!);

describe("charWidth", () => {
  it("is one column for ordinary characters", () => {
    for (const ch of ["a", "é", "→", "❤", "▶", "⌨"]) expect(width(ch)).toBe(1);
  });

  it("is two for wide CJK and what shows as emoji", () => {
    for (const ch of ["日", "한", "Ａ", "　", "😀", "🚀", "⚡", "✅", "⭐", "🩰", "⌚"]) expect(width(ch)).toBe(2);
  });

  it("is none for a mark on the character before", () => {
    // A combining accent, the joiner, both variation selectors, a zero-width space.
    for (const cp of [0x301, 0x200d, 0xfe0f, 0xfe0e, 0x200b, 0x20e3, 0xe0100]) expect(charWidth(cp)).toBe(0);
  });

  it("counts a flag's letters one each, as the terminal does", () => {
    expect(charWidth(0x1f1fa)).toBe(1);
  });
});

describe("textWidth", () => {
  it("counts a cell once, whatever is stacked on it", () => {
    expect(textWidth("é")).toBe(1);
    expect(textWidth("❤️")).toBe(1);
    // A family: three people, two columns each, and two joiners that take none.
    expect(textWidth("👨‍👩‍👧")).toBe(6);
    expect(textWidth("go 🚀 now")).toBe(9);
  });
});

describe("rowRuns", () => {
  it("fills the gap after a character with a mark on it to the right column", () => {
    const row: Row = {
      y: 0,
      spans: [
        { x: 0, text: "café" },
        { x: 6, text: "ok" },
      ],
    };
    expect(rowRuns(row).map((r) => r.text)).toEqual(["café", "  ", "ok"]);
  });

  it("fills the gap after an emoji outside the old ranges", () => {
    const row: Row = {
      y: 0,
      spans: [
        { x: 0, text: "🚀" },
        { x: 3, text: "x" },
      ],
    };
    expect(rowRuns(row).map((r) => r.text)).toEqual(["🚀", " ", "x"]);
  });
});
