import { describe, expect, it } from "vitest";
import type { Modes } from "../proto";
import { cellAt, mouseBytes, wantsMouse, type MouseReport } from "./mouse";

const modes = (m: Partial<Modes> = {}): Modes => ({ app_cursor: false, bracketed_paste: false, alt_screen: false, ...m });
const text = (bytes: number[] | null) => (bytes === null ? null : String.fromCharCode(...bytes));
const at = (action: MouseReport["action"], x: number, y: number, more: Partial<MouseReport> = {}): MouseReport => ({
  action,
  button: 0,
  x,
  y,
  ...more,
});

// The same cases as the TUI's `reports_are_encoded_as_the_program_asked`.
describe("mouseBytes", () => {
  const sgr = modes({ mouse_click: true, mouse_sgr: true });

  it("reports a press and a release in the SGR encoding", () => {
    expect(text(mouseBytes(at("press", 4, 6), sgr))).toBe("\x1b[<0;5;7M");
    expect(text(mouseBytes(at("release", 4, 6), sgr))).toBe("\x1b[<0;5;7m");
    expect(text(mouseBytes(at("press", 4, 6, { button: 2 }), sgr))).toBe("\x1b[<2;5;7M");
    expect(text(mouseBytes(at("release", 4, 6, { button: 1 }), sgr))).toBe("\x1b[<1;5;7m");
  });

  it("gives a program only the kind of report it asked for", () => {
    expect(mouseBytes(at("drag", 0, 0), sgr)).toBeNull();
    const drags = modes({ mouse_drag: true, mouse_sgr: true });
    expect(text(mouseBytes(at("drag", 0, 0), drags))).toBe("\x1b[<32;1;1M");
    expect(mouseBytes(at("move", 0, 0), drags)).toBeNull();
    const all = modes({ mouse_motion: true, mouse_sgr: true });
    expect(text(mouseBytes(at("move", 2, 3), all))).toBe("\x1b[<35;3;4M");
    expect(text(mouseBytes(at("drag", 2, 3, { button: 2 }), all))).toBe("\x1b[<34;3;4M");
    expect(mouseBytes(at("press", 0, 0), modes())).toBeNull();
    expect(mouseBytes(at("press", 0, 0), undefined)).toBeNull();
  });

  it("reports the wheel and the keys held", () => {
    expect(text(mouseBytes(at("wheel-up", 9, 0), sgr))).toBe("\x1b[<64;10;1M");
    expect(text(mouseBytes(at("wheel-down", 9, 0), sgr))).toBe("\x1b[<65;10;1M");
    expect(text(mouseBytes(at("press", 0, 0, { ctrl: true }), sgr))).toBe("\x1b[<16;1;1M");
    expect(text(mouseBytes(at("press", 0, 0, { alt: true, shift: true }), sgr))).toBe("\x1b[<12;1;1M");
  });

  it("falls back to the old encoding, where a release is button 3", () => {
    const x10 = modes({ mouse_click: true });
    expect(text(mouseBytes(at("press", 4, 6), x10))).toBe("\x1b[M %'");
    expect(text(mouseBytes(at("release", 4, 6), x10))).toBe("\x1b[M#%'");
    expect(mouseBytes(at("press", 300, 0), x10)).toBeNull();
    // 1005 reaches further: the coordinate is a UTF-8 character (U+014D here).
    const utf8 = modes({ mouse_click: true, mouse_utf8: true });
    expect(mouseBytes(at("press", 300, 0), utf8)).toEqual([0x1b, 0x5b, 0x4d, 0x20, 0xc5, 0x8d, 0x21]);
    expect(mouseBytes(at("press", 3000, 0), utf8)).toBeNull();
  });
});

describe("wantsMouse", () => {
  it("is any of the three modes", () => {
    expect(wantsMouse(modes())).toBe(false);
    expect(wantsMouse(undefined)).toBe(false);
    // An encoding alone asks for nothing.
    expect(wantsMouse(modes({ mouse_sgr: true }))).toBe(false);
    for (const m of ["mouse_click", "mouse_drag", "mouse_motion"] as const) expect(wantsMouse(modes({ [m]: true }))).toBe(true);
  });
});

describe("cellAt", () => {
  it("finds the cell under a point", () => {
    expect(cellAt(0, 0, 8, 17, 80, 24)).toEqual({ x: 0, y: 0 });
    expect(cellAt(8 * 4 + 7.9, 17 * 6 + 1, 8, 17, 80, 24)).toEqual({ x: 4, y: 6 });
  });

  it("keeps a point outside the grid on its nearest edge", () => {
    expect(cellAt(-30, 9999, 8, 17, 80, 24)).toEqual({ x: 0, y: 23 });
    expect(cellAt(9999, -5, 8, 17, 80, 24)).toEqual({ x: 79, y: 0 });
  });

  it("is nothing before the grid is measured", () => {
    expect(cellAt(10, 10, 0, 17, 80, 24)).toBeNull();
  });
});
