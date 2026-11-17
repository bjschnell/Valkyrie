// The command palette (#183): every session, page, doc and backlog item, and the
// things you'd otherwise click for, found by typing. Ctrl+K opens it (Cmd+K on a
// Mac); in a session's terminal Ctrl+K is the program's (readline's kill-line),
// so there it takes Ctrl+Shift+K, which works everywhere.

import type { Decision, Doc, Item, OrderProposal, SessionInfo } from "../proto";
import { byPosition, isAscii, type KeyLike } from "./termkeys";

/** Whether a key press opens the palette. `terminal`: a session's terminal has the keyboard. */
export function paletteKey(e: KeyLike, mac: boolean, terminal: boolean): boolean {
  if (e.altKey || e.altGraph) return false;
  const own = /^[a-z]$/i.test(e.key) ? e.key : isAscii(e.key) ? "" : (byPosition(e) ?? "");
  if (own.toLowerCase() !== "k") return false;
  if (e.ctrlKey && e.shiftKey && !e.metaKey) return true;
  if (mac) return e.metaKey && !e.ctrlKey && !e.shiftKey;
  return e.ctrlKey && !e.metaKey && !e.shiftKey && !terminal;
}

/** What choosing an entry does. */
export type Do =
  | { go: string }
  | { sheet: true }
  | { look: string }
  | { decision: Decision }
  | { item: number };

export type Group = "Actions" | "Sessions" | "Pages" | "Backlog" | "Docs" | "Themes";

export interface Entry {
  key: string;
  group: Group;
  label: string;
  sub?: string;
  /** More words it's found by, not shown. */
  also?: string;
  do: Do;
}

export interface Page {
  label: string;
  hash: string;
}

export interface Sources {
  sessions: SessionInfo[];
  /** Sessions that need you, first first. */
  needs: number[];
  proposals: Decision[];
  items: Item[];
  orders: OrderProposal[];
  docs: Doc[];
  pages: Page[];
  /** Every look, by its key, and the one in use. */
  looks: { key: string; name: string; about: string }[];
  look: string;
}

const base = (path: string) => path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || path;

/** Every entry, in the order an empty query shows them. */
export function entries(s: Sources): Entry[] {
  const out: Entry[] = [];
  const name = (id: number) => s.sessions.find((x) => x.id === id)?.name ?? `#${id}`;
  if (s.needs.length) {
    out.push({
      key: "needs",
      group: "Actions",
      label: "Go to who needs you",
      sub: name(s.needs[0]),
      also: "next waiting input",
      do: { go: `#/s/${s.needs[0]}` },
    });
  }
  // What only moves you comes first, so Enter on an unsearched palette never accepts.
  out.push(
    { key: "new-session", group: "Actions", label: "New session", also: "start launch agent", do: { go: "#/new" } },
    { key: "new-item", group: "Actions", label: "New backlog item", also: "add todo", do: { go: "#/backlog/new" } },
    { key: "shortcuts", group: "Actions", label: "Keyboard shortcuts", also: "keys help ?", do: { sheet: true } },
  );
  for (const d of s.proposals.filter((d) => d.status === "proposed")) {
    out.push({
      key: `decision-${d.project}-${d.id}`,
      group: "Actions",
      label: `Accept ${d.kind}: ${d.title}`,
      sub: `#${d.id} · ${base(d.project)} · by ${d.provenance.by}`,
      also: "proposal decision",
      do: { decision: d },
    });
  }
  for (const i of s.items.filter((i) => i.status === "proposed")) {
    out.push({
      key: `accept-item-${i.id}`,
      group: "Actions",
      label: `Accept #${i.number}: ${i.title}`,
      sub: `${i.kind} · by ${i.by}`,
      also: "proposal backlog item",
      do: { item: i.id },
    });
  }
  for (const o of s.orders) {
    out.push({
      key: `order-${o.id}`,
      group: "Actions",
      label: `Review build order: ${base(o.project)}`,
      sub: o.trigger,
      also: "proposal backlog",
      do: { go: `#/backlog/order/${encodeURIComponent(o.project)}` },
    });
  }
  for (const x of s.sessions) {
    out.push({
      key: `session-${x.id}`,
      group: "Sessions",
      label: x.name,
      sub: [`#${x.id}`, base(x.cwd), x.report].filter(Boolean).join(" · "),
      also: x.title ?? undefined,
      do: { go: `#/s/${x.id}` },
    });
  }
  for (const p of s.pages) {
    out.push({ key: `page-${p.hash}`, group: "Pages", label: p.label, also: "go page", do: { go: p.hash } });
  }
  for (const i of s.items.filter((i) => i.status === "open" || i.status === "proposed")) {
    out.push({
      key: `item-${i.id}`,
      group: "Backlog",
      label: `#${i.number} ${i.title}`,
      sub: [i.kind, i.status === "proposed" ? "proposed" : null, i.project && base(i.project)].filter(Boolean).join(" · "),
      do: { go: `#/backlog/${i.id}` },
    });
  }
  for (const d of s.docs) {
    if (!d.key) continue;
    out.push({
      key: `doc-${d.key}`,
      group: "Docs",
      label: d.title,
      sub: [d.kind, d.repo && base(d.repo)].filter(Boolean).join(" · "),
      also: d.path,
      do: { go: `#/docs/${encodeURIComponent(d.key)}` },
    });
  }
  for (const l of s.looks) {
    out.push({
      key: `look-${l.key}`,
      group: "Themes",
      label: `Theme: ${l.name}`,
      sub: l.key === s.look ? `${l.about} · current` : l.about,
      also: "look switch colors",
      do: { look: l.key },
    });
  }
  return out;
}

/**
 * How well `text` matches one lowercased word of a query: 0 for not at all.
 * A run of it beats its letters scattered, and at a word's start beats inside one.
 */
function score(text: string, word: string): number {
  const t = text.toLowerCase();
  const at = t.indexOf(word);
  if (at === 0) return 100;
  if (at > 0) return /[\s\-_/:#.·]/.test(t[at - 1]) ? 80 : 50;
  // Its letters in order, closer together scoring higher.
  let from = 0;
  let first = -1;
  for (const ch of word) {
    const i = t.indexOf(ch, from);
    if (i < 0) return 0;
    if (first < 0) first = i;
    from = i + 1;
  }
  return Math.max(1, 30 - (from - first - word.length));
}

/** The entries a query finds, best first; an empty one keeps the order given. */
export function rank(list: Entry[], query: string): Entry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return list;
  const scored: { e: Entry; s: number; i: number }[] = [];
  list.forEach((e, i) => {
    let total = 0;
    for (const w of words) {
      // The label counts most; what's under it and its other words only as substrings.
      const s = Math.max(
        score(e.label, w) * 2,
        e.sub?.toLowerCase().includes(w) ? 40 : 0,
        e.also?.toLowerCase().includes(w) ? 30 : 0,
        e.group.toLowerCase().startsWith(w) ? 30 : 0,
      );
      if (!s) return;
      total += s;
    }
    scored.push({ e, s: total, i });
  });
  scored.sort((a, b) => b.s - a.s || a.i - b.i);
  return scored.map((x) => x.e);
}
