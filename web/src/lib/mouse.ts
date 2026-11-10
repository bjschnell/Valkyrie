// Mouse reports for a program that asked for them (vim with mouse=a, htop,
// lazygit): what a terminal sends for a click, a drag, a move or the wheel, in
// the encoding the program chose. The TUI's `mouse::encode` is the same rules.

import type { Modes } from "../proto";

export type MouseAction = "press" | "release" | "drag" | "move" | "wheel-up" | "wheel-down";

export interface MouseReport {
  action: MouseAction;
  /** 0 left, 1 middle, 2 right; not read for a move or the wheel. */
  button: number;
  /** The cell, from 0. */
  x: number;
  y: number;
  shift?: boolean;
  alt?: boolean;
  ctrl?: boolean;
}

/** The program asked for the mouse (modes 1000, 1002 or 1003). */
export const wantsMouse = (modes?: Modes): boolean =>
  !!modes && (!!modes.mouse_click || !!modes.mouse_drag || !!modes.mouse_motion);

/** The button code of a report: the button, what kind of event, the keys held. */
function code(r: MouseReport): number {
  const base =
    r.action === "wheel-up" ? 64 : r.action === "wheel-down" ? 65 : r.action === "move" ? 35 : r.action === "drag" ? 32 + r.button : r.button;
  return base + (r.shift ? 4 : 0) + (r.alt ? 8 : 0) + (r.ctrl ? 16 : 0);
}

const ESC = 0x1b;

/**
 * The bytes for `r`, or `null` when the program didn't ask for that kind of
 * report (a drag with only clicks on), or its encoding can't reach the cell.
 */
export function mouseBytes(r: MouseReport, modes?: Modes): number[] | null {
  if (!modes) return null;
  const wanted =
    r.action === "move"
      ? !!modes.mouse_motion
      : r.action === "drag"
        ? !!modes.mouse_drag || !!modes.mouse_motion
        : wantsMouse(modes);
  if (!wanted || r.x < 0 || r.y < 0) return null;
  const c = code(r);
  const release = r.action === "release";
  if (modes.mouse_sgr) {
    return [...`\x1b[<${c};${r.x + 1};${r.y + 1}${release ? "m" : "M"}`].map((ch) => ch.charCodeAt(0));
  }
  // The old encoding: a release is "button 3", whichever button it was.
  const legacy = 32 + (release ? (c & ~0b11) | 3 : c);
  if (legacy > 255) return null;
  const out = [ESC, 0x5b, 0x4d, legacy];
  for (const v of [r.x, r.y]) {
    const n = 32 + 1 + v;
    if (modes.mouse_utf8) {
      // 1005: the coordinate as a character, which reaches 2015 cells.
      if (n >= 2048) return null;
      if (n < 0x80) out.push(n);
      else out.push(0xc0 | (n >> 6), 0x80 | (n & 0x3f));
    } else {
      if (n > 255) return null;
      out.push(n);
    }
  }
  return out;
}

/** The cell under a point `px`, `py` from the grid's corner, in cells of `cw` by `ch`. */
export function cellAt(px: number, py: number, cw: number, ch: number, cols: number, rows: number): { x: number; y: number } | null {
  if (!(cw > 0) || !(ch > 0) || cols < 1 || rows < 1) return null;
  // A drag that leaves the grid still reports, from its nearest edge.
  const clamp = (v: number, max: number) => Math.max(0, Math.min(max - 1, Math.floor(v)));
  return { x: clamp(px / cw, cols), y: clamp(py / ch, rows) };
}
