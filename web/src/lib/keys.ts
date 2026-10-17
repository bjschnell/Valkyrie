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
 * A message as typed, without its Enter: pasted (so newlines stay newlines instead
 * of submitting each line) when the program takes bracketed paste. The Enter goes
 * in a later write; Claude Code takes a long chunk of input as a paste, so an
 * Enter arriving with the text becomes a newline and the message is never sent.
 */
export function messageBytes(text: string, modes?: Modes): number[] {
  const body = text.replace(/\r\n?/g, "\n");
  const typed =
    modes?.bracketed_paste && body.includes("\n")
      ? `\x1b[200~${body}\x1b[201~`
      : body.replace(/\n/g, "\r");
  return [...enc.encode(typed)];
}

/**
 * A file's path, for the agent to attach: quoted if it has a space, since Codex
 * reads an unquoted one as text, and pasted when the program takes bracketed
 * paste, so it's read as one (as the TUI pastes a clipboard image).
 */
export function pathBytes(path: string, modes?: Modes): number[] {
  const quoted = /\s/.test(path) ? `"${path}"` : path;
  return [...enc.encode(modes?.bracketed_paste ? `\x1b[200~${quoted}\x1b[201~` : quoted)];
}

export function textBytes(text: string): number[] {
  return [...enc.encode(text)];
}
