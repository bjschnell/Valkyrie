// Bytes for a desktop keyboard's keys, as a terminal would send them to the
// program: the xterm encodings, following the modes the program set.

import type { Modes } from "../proto";

const enc = new TextEncoder();
const bytes = (s: string) => [...enc.encode(s)];

/** The parts of a `KeyboardEvent` the encoding reads. */
export interface KeyLike {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  /** AltGr types a character (`@` on a German layout); it isn't Ctrl+Alt. */
  altGraph?: boolean;
}

/** Keys that end a CSI sequence with a letter: `ESC [ A`, or `ESC [ 1 ; 5 A` modified. */
const LETTER: Record<string, string> = {
  ArrowUp: "A",
  ArrowDown: "B",
  ArrowRight: "C",
  ArrowLeft: "D",
  Home: "H",
  End: "F",
  F1: "P",
  F2: "Q",
  F3: "R",
  F4: "S",
};

/** Keys sent as `ESC [ n ~`. */
const TILDE: Record<string, number> = {
  Insert: 2,
  Delete: 3,
  PageUp: 5,
  PageDown: 6,
  F5: 15,
  F6: 17,
  F7: 18,
  F8: 19,
  F9: 20,
  F10: 21,
  F11: 23,
  F12: 24,
};

/** Ctrl and a key that isn't a letter: `Ctrl+[` is Esc, `Ctrl+2` is NUL, as xterm has them. */
const CTRL: Record<string, number> = {
  " ": 0x00,
  "@": 0x00,
  "2": 0x00,
  "[": 0x1b,
  "3": 0x1b,
  "\\": 0x1c,
  "4": 0x1c,
  "]": 0x1d,
  "5": 0x1d,
  "^": 0x1e,
  "6": 0x1e,
  _: 0x1f,
  "7": 0x1f,
  "/": 0x1f,
  "?": 0x7f,
  "8": 0x7f,
};

/** The character on a key by where it sits, for when a modifier changed what it types. */
function byPosition(e: KeyLike): string | null {
  const letter = /^Key([A-Z])$/.exec(e.code);
  if (letter) return e.shiftKey ? letter[1] : letter[1].toLowerCase();
  const digit = /^Digit([0-9])$/.exec(e.code);
  return digit && !e.shiftKey ? digit[1] : null;
}

/**
 * What a key press sends, or `null` for one the browser or the system keeps (a
 * Cmd chord, a modifier on its own, a dead key still waiting for its letter).
 */
export function encodeKey(e: KeyLike, modes?: Modes): number[] | null {
  if (e.metaKey) return null;
  const altGr = !!e.altGraph;
  const ctrl = e.ctrlKey && !altGr;
  const alt = e.altKey && !altGr;
  const mod = 1 + (e.shiftKey ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0);
  const meta = (s: string) => bytes(alt ? `\x1b${s}` : s);

  switch (e.key) {
    case "Enter":
      // Shift+Enter is a new line in Claude Code and Codex, which read Esc Enter
      // as one; a plain terminal can't tell it from Enter at all.
      return bytes(alt || e.shiftKey ? "\x1b\r" : "\r");
    case "Tab":
      return e.shiftKey ? bytes("\x1b[Z") : meta("\t");
    case "Backspace":
      return meta(ctrl ? "\x08" : "\x7f");
    case "Escape":
      return meta("\x1b");
  }

  const letter = LETTER[e.key];
  if (letter) {
    if (mod > 1) return bytes(`\x1b[1;${mod}${letter}`);
    // F1 to F4 are always SS3; arrows, Home and End follow the cursor-key mode.
    const ss3 = e.key.startsWith("F") || modes?.app_cursor;
    return bytes(`${ss3 ? "\x1bO" : "\x1b["}${letter}`);
  }
  const tilde = TILDE[e.key];
  if (tilde) return bytes(mod > 1 ? `\x1b[${tilde};${mod}~` : `\x1b[${tilde}~`);

  // Anything else with a name ("Shift", "Dead", "Process", "AudioVolumeUp") isn't typed.
  if ([...e.key].length !== 1) return null;

  if (ctrl) {
    // By position first: on a Cyrillic layout Ctrl+С is still Ctrl+C.
    const ch = (byPosition(e) ?? e.key).toLowerCase();
    const code = ch >= "a" && ch <= "z" ? ch.charCodeAt(0) & 0x1f : CTRL[ch];
    if (code === undefined) return null;
    return alt ? [0x1b, code] : [code];
  }
  // Option on a Mac types another character (`∫` for b); Alt+b is what's meant.
  if (alt) return bytes(`\x1b${byPosition(e) ?? e.key}`);
  return bytes(e.key);
}

const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";

/**
 * Pasted text: between the paste marks when the program takes them, so its
 * newlines don't run each line; typed line by line otherwise.
 */
export function pasteBytes(text: string, modes?: Modes): number[] {
  const body = text.replace(/\r\n?/g, "\n");
  if (!modes?.bracketed_paste) return bytes(body.replace(/\n/g, "\r"));
  // Text holding the end mark would close the paste early and type the rest.
  return bytes(`${PASTE_START}${body.replaceAll(PASTE_END, "").replace(/\n/g, "\r")}${PASTE_END}`);
}

/** Tells a program that asked (mode 1004) the terminal gained or lost the keyboard. */
export function focusBytes(focused: boolean, modes?: Modes): number[] | null {
  if (!modes?.focus_events) return null;
  return bytes(focused ? "\x1b[I" : "\x1b[O");
}

/** A wheel turn in the alternate screen, where mode 1007 makes it arrow keys. */
export function wheelBytes(lines: number, modes?: Modes): number[] | null {
  if (!modes?.alt_screen || !modes.alt_scroll || lines === 0) return null;
  const csi = modes.app_cursor ? "\x1bO" : "\x1b[";
  return bytes(`${csi}${lines < 0 ? "A" : "B"}`.repeat(Math.min(Math.abs(lines), 20)));
}

/** Text selected off the grid: without the padding that fills each row out. */
export function copiedText(selected: string): string {
  return selected
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .replace(/\n+$/, "");
}

/** The grid that fits a pane of `width` by `height` pixels, in cells of `cw` by `ch`. */
export function fitSize(width: number, height: number, cw: number, ch: number): { cols: number; rows: number } | null {
  if (!(cw > 0) || !(ch > 0)) return null;
  const cols = Math.floor(width / cw);
  const rows = Math.floor(height / ch);
  // A pane too small to work in (mid-layout, or collapsed) resizes nothing.
  if (cols < 20 || rows < 5) return null;
  return { cols: Math.min(cols, 500), rows: Math.min(rows, 200) };
}
