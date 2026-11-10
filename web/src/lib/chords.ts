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
  const own = (isAscii(e.key) ? e.key : (byPosition(e) ?? "")).toLowerCase();
  if (own >= "1" && own <= "9") return { go: "tab", n: Number(own) };
  return LETTERS[own] ?? null;
}

/** Where a chord leads: a session, home, or nowhere (`undefined`). */
export function chordTarget(
  c: Chord,
  at: { sessions: number[]; needs: number[]; open: number | null; last: number | null },
): { session: number } | "home" | undefined {
  const { sessions, needs, open, last } = at;
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
    case "last":
      return last !== null && last !== open && sessions.includes(last) ? { session: last } : undefined;
    case "tab": {
      const id = sessions[c.n - 1];
      return id === undefined || id === open ? undefined : { session: id };
    }
  }
}
