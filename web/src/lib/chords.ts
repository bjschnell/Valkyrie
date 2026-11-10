// The Alt chords that move between sessions on a desktop keyboard: the TUI's own
// (docs/keys.md), so the same fingers work in both.

import { byPosition, isAscii, type KeyLike } from "./termkeys";

export type Chord =
  | { go: "next" }
  | { go: "prev" }
  | { go: "needs" }
  | { go: "home" }
  | { go: "last" }
  | { go: "tab"; n: number };

const LETTERS: Record<string, Chord> = {
  j: { go: "next" },
  k: { go: "prev" },
  n: { go: "needs" },
  h: { go: "home" },
  p: { go: "last" },
};

/** The chord a key press is, or `null` for a key that's typed or the program's. */
export function chord(e: KeyLike, mac = false): Chord | null {
  if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || e.altGraph) return null;
  if ([...e.key].length !== 1) return null;
  // On a Mac, Option is how some layouts type @ [ ] { } | ~: those stay typed.
  if (mac && isAscii(e.key) && !/[a-z0-9]/i.test(e.key)) return null;
  // A digit is one on the digit row: Alt and the number pad types a character
  // by its code on Windows. AZERTY has punctuation there, so those go by position.
  if (e.code.startsWith("Digit")) {
    const digit = /^[0-9]$/.test(e.key) ? e.key : (byPosition(e) ?? "");
    return digit >= "1" && digit <= "9" ? { go: "tab", n: Number(digit) } : null;
  }
  const own = /^[a-z]$/i.test(e.key) ? e.key : isAscii(e.key) ? "" : (byPosition(e) ?? "");
  return LETTERS[own.toLowerCase()] ?? null;
}

/** Where a chord leads: a session, home, or nowhere (`undefined`). */
export function chordTarget(
  c: Chord,
  at: { sessions: number[]; needs: number[]; open: number | null; recent: number[] },
): { session: number } | "home" | undefined {
  const { sessions, needs, open, recent } = at;
  const step = (list: number[], by: 1 | -1) => {
    if (!list.length) return undefined;
    const i = open === null ? -1 : list.indexOf(open);
    const next = i < 0 ? (by > 0 ? 0 : list.length - 1) : (i + by + list.length) % list.length;
    return list[next] === open ? undefined : { session: list[next] };
  };
  switch (c.go) {
    case "home":
      return open === null ? undefined : "home";
    case "next":
      return step(sessions, 1);
    case "prev":
      return step(sessions, -1);
    case "needs":
      return step(needs, 1);
    case "last": {
      // The latest session you were on that isn't this one and still exists.
      const last = recent.findLast((id) => id !== open && sessions.includes(id));
      return last === undefined ? undefined : { session: last };
    }
    case "tab": {
      const id = sessions[c.n - 1];
      return id === undefined || id === open ? undefined : { session: id };
    }
  }
}
