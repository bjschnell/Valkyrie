import { describe, expect, it } from "vitest";
import type { Modes } from "../proto";
import { copiedRows, copiedText, encodeKey, fitSize, focusBytes, pasteBytes, wheelBytes, type KeyLike } from "./termkeys";

const text = (bytes: number[] | null) => (bytes === null ? null : new TextDecoder().decode(new Uint8Array(bytes)));
const modes = (m: Partial<Modes> = {}): Modes => ({ app_cursor: false, bracketed_paste: false, alt_screen: false, ...m });

/** A key press: `key("c", "KeyC", "ctrl")`. */
function key(k: string, code = "", ...mods: ("ctrl" | "alt" | "shift" | "meta" | "altgr")[]): KeyLike {
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

describe("encodeKey", () => {
  it("types characters as UTF-8", () => {
    expect(text(encodeKey(key("a", "KeyA")))).toBe("a");
    expect(text(encodeKey(key("A", "KeyA", "shift")))).toBe("A");
    expect(encodeKey(key("é"))).toEqual([0xc3, 0xa9]);
    expect(text(encodeKey(key("😀")))).toBe("😀");
  });

  it("sends Ctrl-letters as control codes", () => {
    expect(encodeKey(key("c", "KeyC", "ctrl"))).toEqual([0x03]);
    expect(encodeKey(key("w", "KeyW", "ctrl"))).toEqual([0x17]);
    expect(encodeKey(key("D", "KeyD", "ctrl", "shift"))).toEqual([0x04]);
    expect(encodeKey(key(" ", "Space", "ctrl"))).toEqual([0x00]);
    expect(encodeKey(key("[", "BracketLeft", "ctrl"))).toEqual([0x1b]);
    expect(encodeKey(key("_", "Minus", "ctrl", "shift"))).toEqual([0x1f]);
  });

  it("reads Ctrl-letters by position on a layout with no Latin letters", () => {
    expect(encodeKey(key("с", "KeyC", "ctrl"))).toEqual([0x03]);
  });

  it("takes a Latin layout's letter from the key, wherever it sits", () => {
    // Dvorak: C is where QWERTY has I. AZERTY: A is where QWERTY has Q.
    expect(encodeKey(key("c", "KeyI", "ctrl"))).toEqual([0x03]);
    expect(encodeKey(key("a", "KeyQ", "ctrl"))).toEqual([0x01]);
    expect(text(encodeKey(key("b", "KeyN", "alt")))).toBe("\x1bb");
  });

  it("leaves Ctrl chords with no control code to the browser", () => {
    expect(encodeKey(key("=", "Equal", "ctrl"))).toBeNull();
    expect(encodeKey(key("0", "Digit0", "ctrl"))).toBeNull();
  });

  it("prefixes Alt with Esc, by the key's position", () => {
    expect(text(encodeKey(key("b", "KeyB", "alt")))).toBe("\x1bb");
    expect(text(encodeKey(key("∫", "KeyB", "alt")))).toBe("\x1bb");
    expect(text(encodeKey(key(".", "Period", "alt")))).toBe("\x1b.");
    expect(text(encodeKey(key("Backspace", "Backspace", "alt")))).toBe("\x1b\x7f");
    expect(encodeKey(key("c", "KeyC", "ctrl", "alt"))).toEqual([0x1b, 0x03]);
  });

  it("types what Option types on a Mac layout that needs it", () => {
    expect(text(encodeKey(key("@", "KeyL", "alt"), undefined, { mac: true }))).toBe("@");
    expect(text(encodeKey(key("|", "Digit7", "alt"), undefined, { mac: true }))).toBe("|");
    expect(text(encodeKey(key("∫", "KeyB", "alt"), undefined, { mac: true }))).toBe("\x1bb");
    expect(text(encodeKey(key(".", "Period", "alt"), undefined, { mac: true }))).toBe("\x1b.");
  });

  it("types what AltGr types", () => {
    expect(text(encodeKey(key("@", "KeyQ", "ctrl", "alt", "altgr")))).toBe("@");
  });

  it("follows the cursor-key mode for arrows, Home and End", () => {
    expect(text(encodeKey(key("ArrowUp"), modes()))).toBe("\x1b[A");
    expect(text(encodeKey(key("ArrowUp"), modes({ app_cursor: true })))).toBe("\x1bOA");
    expect(text(encodeKey(key("End"), modes({ app_cursor: true })))).toBe("\x1bOF");
    expect(text(encodeKey(key("Home")))).toBe("\x1b[H");
  });

  it("encodes modifiers on cursor and function keys", () => {
    expect(text(encodeKey(key("ArrowLeft", "", "ctrl"), modes({ app_cursor: true })))).toBe("\x1b[1;5D");
    expect(text(encodeKey(key("ArrowRight", "", "shift", "alt")))).toBe("\x1b[1;4C");
    expect(text(encodeKey(key("Delete", "", "ctrl")))).toBe("\x1b[3;5~");
    expect(text(encodeKey(key("F1")))).toBe("\x1bOP");
    expect(text(encodeKey(key("F1", "", "shift")))).toBe("\x1b[1;2P");
    expect(text(encodeKey(key("F5")))).toBe("\x1b[15~");
    expect(text(encodeKey(key("F12", "", "ctrl")))).toBe("\x1b[24;5~");
    expect(text(encodeKey(key("F10")))).toBe("\x1b[21~");
    expect(encodeKey(key("F11"))).toBeNull();
    expect(encodeKey(key("F12"))).toBeNull();
    expect(text(encodeKey(key("PageUp")))).toBe("\x1b[5~");
  });

  it("encodes the editing keys", () => {
    expect(text(encodeKey(key("Enter")))).toBe("\r");
    expect(text(encodeKey(key("Enter", "", "shift")))).toBe("\r");
    expect(text(encodeKey(key("Enter", "", "shift"), undefined, { agent: true }))).toBe("\x1b\r");
    expect(text(encodeKey(key("Enter", "", "alt")))).toBe("\x1b\r");
    expect(text(encodeKey(key("Tab")))).toBe("\t");
    expect(text(encodeKey(key("Tab", "", "shift")))).toBe("\x1b[Z");
    expect(text(encodeKey(key("Backspace")))).toBe("\x7f");
    expect(text(encodeKey(key("Backspace", "", "ctrl")))).toBe("\x08");
    expect(text(encodeKey(key("Escape")))).toBe("\x1b");
  });

  it("keeps out of Cmd chords, bare modifiers and unfinished keys", () => {
    expect(encodeKey(key("v", "KeyV", "meta"))).toBeNull();
    expect(encodeKey(key("Shift", "ShiftLeft", "shift"))).toBeNull();
    expect(encodeKey(key("Dead", "Backquote"))).toBeNull();
    expect(encodeKey(key("Process"))).toBeNull();
  });
});

describe("pasteBytes", () => {
  it("marks a paste when the program takes bracketed paste", () => {
    expect(text(pasteBytes("a\r\nb", modes({ bracketed_paste: true })))).toBe("\x1b[200~a\rb\x1b[201~");
  });

  it("types the lines otherwise", () => {
    expect(text(pasteBytes("a\nb"))).toBe("a\rb");
  });

  it("lets no end mark through, nested or not", () => {
    const marked = modes({ bracketed_paste: true });
    expect(text(pasteBytes("ok\x1b[201~rm -rf", marked))).toBe("\x1b[200~ok[201~rm -rf\x1b[201~");
    const nested = text(pasteBytes("ok\x1b[20\x1b[201~1~rm -rf\n", marked))!;
    expect(nested.indexOf("\x1b[201~")).toBe(nested.length - 6);
    expect(nested.slice(6, -6)).not.toContain("\x1b");
  });

  it("types no Esc from unmarked text either", () => {
    expect(text(pasteBytes("a\x1b[Ab"))).toBe("a[Ab");
  });
});

describe("focusBytes", () => {
  it("reports focus only to a program that asked", () => {
    expect(focusBytes(true, modes())).toBeNull();
    expect(text(focusBytes(true, modes({ focus_events: true })))).toBe("\x1b[I");
    expect(text(focusBytes(false, modes({ focus_events: true })))).toBe("\x1b[O");
  });
});

describe("wheelBytes", () => {
  it("turns the wheel into arrows in the alternate screen", () => {
    const alt = modes({ alt_screen: true, alt_scroll: true });
    expect(text(wheelBytes(-2, alt))).toBe("\x1b[A\x1b[A");
    expect(text(wheelBytes(1, { ...alt, app_cursor: true }))).toBe("\x1bOB");
  });

  it("sends nothing on the main screen or without the mode", () => {
    expect(wheelBytes(-1, modes({ alt_scroll: true }))).toBeNull();
    expect(wheelBytes(-1, modes({ alt_screen: true }))).toBeNull();
  });
});

describe("copiedText", () => {
  it("drops the padding at the end of rows and the blank rows after", () => {
    expect(copiedText("ls   \n  a  b   \n   \n")).toBe("ls\n  a  b");
  });
});

describe("copiedRows", () => {
  const row = (text: string, wrapped = false, missing = 0) => ({ text, wrapped, missing });

  it("joins a row the program wrapped to the next, blanks kept", () => {
    expect(copiedRows([row("echo hello", true, 1), row("world   "), row("$ ")])).toBe("echo hello world\n$");
    expect(copiedRows([row("aaaa", true), row("bbbb", true), row("cc")])).toBe("aaaabbbbcc");
  });

  it("breaks the line after any other row", () => {
    expect(copiedRows([row("ls   "), row(" "), row("a  b  ")])).toBe("ls\n\na  b");
  });

  it("ends at the selection, on a wrapped row too", () => {
    expect(copiedRows([row("tail", true, 3)])).toBe("tail");
    expect(copiedRows([row("a"), row(" "), row("")])).toBe("a");
  });
});

describe("fitSize", () => {
  it("fits whole cells", () => {
    expect(fitSize(805, 410, 8, 17)).toEqual({ cols: 100, rows: 24 });
  });

  it("leaves a pane too small to use alone", () => {
    expect(fitSize(100, 400, 8, 17)).toBeNull();
    expect(fitSize(800, 400, 0, 17)).toBeNull();
  });
});
