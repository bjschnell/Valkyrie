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

export interface KeyOptions {
  /** A Mac keyboard: Option types characters there, and nothing says AltGr. */
  mac?: boolean;
  /** The program is an agent that takes Esc Enter as a new line in its message. */
  agent?: boolean;
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

/**
 * The character on a key by where it sits on a US keyboard. Only for a key whose
 * own character can't be used: a Cyrillic or Greek letter under Ctrl, or what
 * Option turned a letter into on a Mac. A Latin layout's letter is the key's own,
 * wherever it sits (Ctrl+C on Dvorak is the key marked C).
 */
function byPosition(e: KeyLike): string | null {
  const letter = /^Key([A-Z])$/.exec(e.code);
  if (letter) return e.shiftKey ? letter[1] : letter[1].toLowerCase();
  const digit = /^Digit([0-9])$/.exec(e.code);
  return digit && !e.shiftKey ? digit[1] : null;
}

const isAscii = (ch: string) => ch.length === 1 && ch.charCodeAt(0) < 0x80;

/**
 * What a key press sends, or `null` for one the browser or the system keeps (a
 * Cmd chord, a modifier on its own, a dead key still waiting for its letter).
 */
export function encodeKey(e: KeyLike, modes?: Modes, options: KeyOptions = {}): number[] | null {
  if (e.metaKey) return null;
  const altGr = !!e.altGraph;
  const ctrl = e.ctrlKey && !altGr;
  const alt = e.altKey && !altGr;
  const mod = 1 + (e.shiftKey ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0);
  const meta = (s: string) => bytes(alt ? `\x1b${s}` : s);

  switch (e.key) {
    case "Enter":
      // Shift+Enter is a new line in Claude Code and Codex, which read Esc Enter
      // as one. Only for them: to bash Esc Enter switches to vi editing, and in
      // vim it leaves insert mode. A terminal sends Enter for it, so that's the rest.
      return bytes(alt || (e.shiftKey && options.agent) ? "\x1b\r" : "\r");
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
  // F11 is the browser's full screen and F12 its tools; nothing else would reach them.
  if ((e.key === "F11" || e.key === "F12") && mod === 1) return null;
  const tilde = TILDE[e.key];
  if (tilde) return bytes(mod > 1 ? `\x1b[${tilde};${mod}~` : `\x1b[${tilde}~`);

  // Anything else with a name ("Shift", "Dead", "Process", "AudioVolumeUp") isn't typed.
  if ([...e.key].length !== 1) return null;

  // The key's own character, unless it's one a control code or Meta can't carry.
  const own = isAscii(e.key) ? e.key : (byPosition(e) ?? e.key);
  if (ctrl) {
    const ch = own.toLowerCase();
    const code = ch >= "a" && ch <= "z" ? ch.charCodeAt(0) & 0x1f : CTRL[ch];
    if (code === undefined) return null;
    return alt ? [0x1b, code] : [code];
  }
  if (alt) {
    // On a Mac, Option is how a German or French layout types @ [ ] { } | ~ \:
    // those are typed. Its other characters (`∫` for b) stand for Alt and the key.
    if (options.mac && isAscii(e.key) && !/[a-z0-9]/i.test(e.key) && /^(Key|Digit)/.test(e.code)) {
      return bytes(e.key);
    }
    return bytes(`\x1b${own}`);
  }
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
  // No Esc gets through: text holding the end mark would close the paste early and
  // type the rest, and taking the mark out can leave another (kitty and foot drop
  // Esc too). Unmarked, an Esc would start a key sequence of the text's choosing.
  const typed = body.replace(/\x1b/g, "").replace(/\n/g, "\r");
  return bytes(modes?.bracketed_paste ? `${PASTE_START}${typed}${PASTE_END}` : typed);
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

/** One row of a selection off the grid. */
export interface CopiedRow {
  /** The selected part of the row. */
  text: string;
  /** The program wrapped this row onto the next: they are one line. */
  wrapped: boolean;
  /** Blank cells between the row's last character and the right edge. */
  missing: number;
}

/**
 * Text selected off the grid, row by row: a row the program soft-wrapped runs
 * into the next with no line break and keeps its blanks (`hello ` + `world`),
 * as the TUI copies it.
 */
export function copiedRows(rows: CopiedRow[]): string {
  let out = "";
  rows.forEach((row, i) => {
    const last = i === rows.length - 1;
    if (row.wrapped && !last) out += row.text + " ".repeat(Math.max(0, row.missing));
    else out += row.text.trimEnd() + (last ? "" : "\n");
  });
  // The last row is trimmed already; this takes a wrapped row's blanks too when
  // nothing of the row after it was selected.
  return out.trimEnd();
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
