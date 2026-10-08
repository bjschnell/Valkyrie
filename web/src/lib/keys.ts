// Bytes for keys a phone keyboard lacks, and for text typed in the composer.

import type { Modes } from "../proto";

const enc = new TextEncoder();

export type Key = "esc" | "tab" | "shift_tab" | "up" | "down" | "left" | "right" | "ctrl_c" | "enter";

export function keyBytes(key: Key, modes?: Modes): number[] {
  // Arrows follow the program's cursor-key mode, as a terminal would.
  const csi = modes?.app_cursor ? "\x1bO" : "\x1b[";
  const seq: Record<Key, string> = {
    esc: "\x1b",
    tab: "\t",
    shift_tab: "\x1b[Z",
    up: `${csi}A`,
    down: `${csi}B`,
    right: `${csi}C`,
    left: `${csi}D`,
    ctrl_c: "\x03",
    enter: "\r",
  };
  return [...enc.encode(seq[key])];
}

/**
 * A message as typed: pasted (so newlines stay newlines instead of submitting each
 * line) when the program takes bracketed paste, then Enter.
 */
export function messageBytes(text: string, modes?: Modes): number[] {
  const body = text.replace(/\r\n?/g, "\n");
  const typed =
    modes?.bracketed_paste && body.includes("\n")
      ? `\x1b[200~${body}\x1b[201~`
      : body.replace(/\n/g, "\r");
  return [...enc.encode(typed), ...enc.encode("\r")];
}

export function textBytes(text: string): number[] {
  return [...enc.encode(text)];
}
