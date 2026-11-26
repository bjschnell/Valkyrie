// The keys the web app answers to (#189), said once so the '?' sheet can show
// them and, later, the command palette can offer them. A page's plain keys are
// PAGE_KEYS, and the pages bind them with usePageKeys (lib/pageKeys.ts),
// which TypeScript holds to exactly this table: a key bound but not listed, or
// listed but not bound, doesn't compile. What a plain key can't say (Ctrl chords,
// a composer's own keys) is in ALSO, by hand.

import type { KeyLike } from "./termkeys";

/** One thing a key does. `keys` are alternatives; each is the caps to press, in order. */
export interface Shortcut {
  keys: string[][];
  does: string;
}

export interface Section {
  title: string;
  shortcuts: Shortcut[];
}

/** The pages a route can be (App's `Route["page"]`). */
export type PageKey =
  | "inbox"
  | "new"
  | "start"
  | "settings"
  | "backlog"
  | "decisions"
  | "docs"
  | "past"
  | "wiki"
  | "supervisor"
  | "doc"
  | "item"
  | "session"
  | "review";

/** Whether a key press toggles the sheet: a bare `?`, never while you're typing. */
export function shortcutsKey(e: KeyLike, target: Element | null): boolean {
  if (e.key !== "?" || e.ctrlKey || e.metaKey || e.altKey || e.altGraph) return false;
  return !target?.closest?.("textarea, input, select, [contenteditable]");
}

/** Keys that work wherever you are: the same fingers as the TUI's (docs/keys.md). */
function everywhere(mac: boolean): Section {
  const alt = mac ? "Option" : "Alt";
  const mod = mac ? "⌘" : "Ctrl";
  return {
    title: "Everywhere",
    shortcuts: [
      { keys: [[mod, "K"], ...(mac ? [] : [["Ctrl", "Shift", "K"]])], does: "Command palette (Ctrl+Shift+K from a terminal)" },
      { keys: [["?"]], does: "This sheet (from a terminal, through the palette)" },
      { keys: [[alt, "J"], [alt, "K"]], does: "Next, previous session" },
      { keys: [[alt, "N"]], does: "Go to whoever needs you" },
      { keys: [[alt, "P"]], does: "Back to the last session" },
      { keys: [[alt, "H"]], does: "Home" },
      { keys: [[alt, "1…9"]], does: "Jump to that session" },
      { keys: [["g", "i"], ["g", "b"], ["g", "m"], ["g", "h"]], does: "Inbox, Backlog, Memory, History (not in a session)" },
    ],
  };
}

const ctrlEnter = (mac: boolean, does: string): Shortcut => ({ keys: [[mac ? "⌘" : "Ctrl", "Enter"]], does });

/**
 * The plain keys of each page, by the `KeyboardEvent.key` they arrive as, and what
 * each does. Keys that share a description show as one row ("j or k").
 */
export const PAGE_KEYS = {
  inbox: {
    j: "Pick the next, previous item or session",
    k: "Pick the next, previous item or session",
    Enter: "Open it",
    o: "Open it",
    Escape: "Close the picked item, back to the composer",
    r: "Review its changes",
    n: "New session: the composer beside the list (Enter starts the task in its own worktree, Esc leaves it and keeps what's typed), or the launcher",
    b: "Backlog",
    d: "Memory (decisions, skills, policies)",
    a: "What agents did while you were gone",
    h: "History (what happened, to search and resume)",
    s: "Supervisor",
    "/": "Type in the composer when it shows, else search session history",
    ",": "Settings",
  },
  session: {
    j: "Scroll down, up",
    k: "Scroll down, up",
    g: "Top, bottom",
    G: "Top, bottom",
    i: "Write to the agent",
    t: "Switch between chat and terminal",
    r: "Review its changes",
    D: "Docs this session wrote, beside it: j and k pick one (in a doc, scroll), Enter reads it, Esc goes back, then closes",
    q: "Back to the inbox",
  },
  review: {
    j: "Next, previous file",
    k: "Next, previous file",
    o: "Open or close the file",
    r: "Reload",
    i: "Write to the agent",
    q: "Back to the session",
  },
  backlog: {
    j: "Pick the next, previous item in a lane",
    k: "Pick the next, previous item in a lane",
    h: "Pick from the lane to the left, right",
    l: "Pick from the lane to the left, right",
    Enter: "Open or close it",
    Escape: "Close it",
    n: "Write an item down",
    a: "Write an item down",
    q: "Back to the inbox",
  },
  decisions: {
    j: "Pick the next, previous one",
    k: "Pick the next, previous one",
    Enter: "Open or close it",
    Escape: "Close it, or go back",
    "/": "Filter",
    q: "Go back",
  },
  docs: { q: "Back to the inbox", Escape: "Back to the inbox" },
  doc: { q: "Back to the list", Escape: "Back to the list" },
  settings: { q: "Go back", Escape: "Go back" },
  past: {
    j: "Pick the next, previous one",
    k: "Pick the next, previous one",
    Enter: "Open it, resume it, or its PR",
    "/": "Search",
    Escape: "Clear the search, or go back (Esc in the box clears it)",
    q: "Go back",
  },
  wiki: { q: "Go back (from a page, to the list)", Escape: "Go back (from a page, to the list)" },
  supervisor: { q: "Go back", Escape: "Go back", c: "Open the supervisor session" },
} as const satisfies Partial<Record<PageKey, Record<string, string>>>;

/** The pages whose plain keys are in PAGE_KEYS. */
export type KeyedPage = keyof typeof PAGE_KEYS;

const TITLE: Record<PageKey, string> = {
  inbox: "Inbox",
  new: "New session",
  start: "Start agents",
  settings: "Settings",
  backlog: "Backlog",
  decisions: "Memory",
  docs: "Docs",
  past: "History",
  wiki: "Wiki",
  supervisor: "Supervisor",
  doc: "Docs",
  item: "Item",
  session: "Session",
  review: "Review",
};

/** How a key reads on a keycap. */
const CAP: Record<string, string[]> = { Escape: ["Esc"], G: ["Shift", "G"], D: ["Shift", "D"] };

/** What the keys do on `page`, and then what they do everywhere. */
export function sections(page: PageKey, mac: boolean): Section[] {
  const shortcuts = [...plain(page), ...(ALSO[page]?.(mac) ?? [])];
  return [...(shortcuts.length ? [{ title: TITLE[page], shortcuts }] : []), everywhere(mac)];
}

/** A page's PAGE_KEYS as rows: keys with the same words are one row. */
function plain(page: PageKey): Shortcut[] {
  const keys: Record<string, string> | undefined = (PAGE_KEYS as Record<string, Record<string, string>>)[page];
  const rows: Shortcut[] = [];
  for (const [key, does] of Object.entries(keys ?? {})) {
    const cap = CAP[key] ?? [key];
    const row = rows.find((r) => r.does === does);
    if (row) row.keys.push(cap);
    else rows.push({ keys: [cap], does });
  }
  return rows;
}

/** What a plain key can't say, by hand. */
const ALSO: Partial<Record<PageKey, (mac: boolean) => Shortcut[]>> = {
  session: () => [
    { keys: [["Ctrl", "D"], ["Ctrl", "U"]], does: "Scroll half a page" },
    { keys: [["Enter"]], does: "Send, from the composer (Shift+Enter for a new line, Esc to leave it)" },
    { keys: [["Ctrl", "Shift", "Enter"]], does: "Give the keyboard back from the terminal" },
    { keys: [["Ctrl", "Shift", "C"], ["Ctrl", "Shift", "V"]], does: "Copy, paste in the terminal" },
  ],
  item: (mac) => [ctrlEnter(mac, "Save")],
  inbox: (mac) => [
    { keys: [["Shift", "Enter"]], does: "In the composer: another line (several lines open Start agents)" },
    ctrlEnter(mac, "In the composer: file it to the backlog instead"),
    { keys: [["#"]], does: "In the composer: find a backlog item by number or title" },
  ],
  new: (mac) => [ctrlEnter(mac, "Start it")],
  start: (mac) => [
    ctrlEnter(mac, "Start them all"),
    { keys: [["Enter"]], does: "Add the task typed, or on a line: start them all" },
    { keys: [["j"], ["k"]], does: "Next, previous line or backlog item" },
    { keys: [["d"], [mac ? "⌘" : "Ctrl", "Backspace"]], does: "Drop the line" },
    { keys: [["Esc"]], does: "Back to the box, or close and keep the lines in the composer" },
  ],
  doc: (mac) => [ctrlEnter(mac, "Save, when editing")],
};
