// The keys the web app answers to (#189), said once so the '?' sheet can show
// them and, later, the command palette can offer them. The handlers live in the
// pages; keep this list in step when one changes (shortcuts.test.ts checks the
// shared ones against the code that decides them).

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
  | "settings"
  | "backlog"
  | "decisions"
  | "docs"
  | "digest"
  | "past"
  | "search"
  | "supervisor"
  | "doc"
  | "item"
  | "order"
  | "session"
  | "review";

/** Whether a key press toggles the sheet: a bare `?`, never while you're typing. */
export function shortcutsKey(e: KeyLike, target: Element | null): boolean {
  if (e.key !== "?" || e.ctrlKey || e.metaKey || e.altKey || e.altGraph) return false;
  return !target?.closest?.("textarea, input, select, [contenteditable]");
}

const back: Shortcut = { keys: [["q"], ["Esc"]], does: "Go back" };

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
    ],
  };
}

const ctrlEnter = (mac: boolean, does: string): Shortcut => ({ keys: [[mac ? "⌘" : "Ctrl", "Enter"]], does });

/** What the keys do on `page`, and then what they do everywhere. */
export function sections(page: PageKey, mac: boolean): Section[] {
  const here = onPage(page, mac);
  return [...(here.shortcuts.length ? [here] : []), everywhere(mac)];
}

function onPage(page: PageKey, mac: boolean): Section {
  const make = (title: string, shortcuts: Shortcut[]): Section => ({ title, shortcuts });
  switch (page) {
    case "inbox":
      return make("Inbox", [
        { keys: [["j"], ["k"]], does: "Pick the next, previous card or session" },
        { keys: [["Enter"], ["o"]], does: "Open it" },
        { keys: [["r"]], does: "Review its changes" },
        { keys: [["n"]], does: "New session" },
        { keys: [["b"]], does: "Backlog" },
        { keys: [["d"]], does: "Memory (decisions, skills, policies)" },
        { keys: [["a"]], does: "What agents did while you were gone" },
        { keys: [["h"]], does: "Past sessions" },
        { keys: [["s"]], does: "Supervisor" },
        { keys: [["/"]], does: "Search session history" },
        { keys: [[","]], does: "Settings" },
      ]);
    case "session":
      return make("Session", [
        { keys: [["j"], ["k"]], does: "Scroll down, up" },
        { keys: [["Ctrl", "D"], ["Ctrl", "U"]], does: "Scroll half a page" },
        { keys: [["g"], ["Shift", "G"]], does: "Top, bottom" },
        { keys: [["i"]], does: "Write to the agent" },
        { keys: [["t"]], does: "Switch between chat and terminal" },
        { keys: [["r"]], does: "Review its changes" },
        { keys: [["q"]], does: "Back to the inbox" },
        { keys: [["Enter"]], does: "Send, from the composer (Shift+Enter for a new line, Esc to leave it)" },
        { keys: [["Ctrl", "Shift", "Enter"]], does: "Give the keyboard back from the terminal" },
        { keys: [["Ctrl", "Shift", "C"], ["Ctrl", "Shift", "V"]], does: "Copy, paste in the terminal" },
      ]);
    case "review":
      return make("Review", [
        { keys: [["j"], ["k"]], does: "Next, previous file" },
        { keys: [["o"]], does: "Open or close the file" },
        { keys: [["r"]], does: "Reload" },
        { keys: [["i"]], does: "Write to the agent" },
        { keys: [["q"]], does: "Back to the session" },
      ]);
    case "backlog":
      return make("Backlog", [
        { keys: [["n"], ["a"]], does: "Write an item down" },
        { keys: [["q"]], does: "Back to the inbox" },
      ]);
    case "item":
      return make("Item", [ctrlEnter(mac, "Save")]);
    case "new":
      return make("New session", [ctrlEnter(mac, "Start it")]);
    case "order":
      return make("Build order", [{ keys: [["q"], ["Esc"]], does: "Back to the backlog" }]);
    case "decisions":
    case "search":
      return make(page === "search" ? "Search" : "Memory", [
        { keys: [["/"]], does: page === "search" ? "Focus the search box" : "Filter" },
        page === "search" ? { ...back, does: "Go back (Esc in the box just leaves it)" } : back,
      ]);
    case "docs":
    case "doc":
      return make("Docs", [
        { keys: [["q"], ["Esc"]], does: page === "doc" ? "Back to the list" : "Back to the inbox" },
        ctrlEnter(mac, "Save, when editing"),
      ]);
    case "settings":
    case "digest":
    case "past":
    case "supervisor":
      return make(PAGE_TITLE[page], [back]);
  }
}

const PAGE_TITLE = {
  settings: "Settings",
  digest: "Digest",
  past: "Past sessions",
  supervisor: "Supervisor",
} as const;
