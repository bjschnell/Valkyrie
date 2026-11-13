// Search over session history (backlog #106): the words to mark in a hit's
// snippet, and the windows to search. The daemon does the matching.

import type { SearchField } from "../proto";

/** Lower case a character at a time, as the daemon folds (a final sigma stays σ). */
export function fold(text: string): string {
  return Array.from(text, (c) => Array.from(c.toLowerCase())[0] ?? c).join("");
}

/** The query's words as the daemon reads them: lower case, a "quoted phrase" whole. */
export function terms(query: string): string[] {
  const out: string[] = [];
  query.split('"').forEach((part, i) => {
    if (i % 2 === 1) {
      const phrase = part.split(/\s+/).filter(Boolean).join(" ");
      if (phrase) out.push(fold(phrase));
    } else {
      out.push(...part.split(/\s+/).filter(Boolean).map(fold));
    }
  });
  return [...new Set(out)];
}

/** `text` cut into runs, each marked when it is one of the words (any case). */
export function marks(text: string, words: string[]): { text: string; hit: boolean }[] {
  const lower = fold(text);
  // Lower casing can change a string's length; marking needs positions to agree.
  if (lower.length !== text.length || words.length === 0) return [{ text, hit: false }];
  const hit = new Array<boolean>(text.length).fill(false);
  for (const w of words) {
    for (let at = lower.indexOf(w); at >= 0 && w; at = lower.indexOf(w, at + w.length)) {
      hit.fill(true, at, at + w.length);
    }
  }
  const out: { text: string; hit: boolean }[] = [];
  for (let i = 0; i < text.length; ) {
    let j = i;
    while (j < text.length && hit[j] === hit[i]) j++;
    out.push({ text: text.slice(i, j), hit: hit[i] });
    i = j;
  }
  return out;
}

export type SearchWindow = "1d" | "3d" | "1w" | "all";

export const WINDOWS: [SearchWindow, string][] = [
  ["1d", "a day"],
  ["3d", "3 days"],
  ["1w", "a week"],
  ["all", "a month"],
];

/** Where a window starts; `null`: as far back as the daemon keeps looking. */
export function windowStart(w: SearchWindow, now: number): number | null {
  const day = 86_400_000;
  switch (w) {
    case "1d":
      return now - day;
    case "3d":
      return now - 3 * day;
    case "1w":
      return now - 7 * day;
    case "all":
      return null;
  }
}

export function fieldLabel(f: SearchField): string {
  return f === "you" ? "you said" : f === "agent" ? "agent said" : f === "file" ? "wrote" : "about";
}
