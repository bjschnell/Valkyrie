// The Docs view (ADR-0021 §5): agent-level docs, then each repository's, and
// [[wikilinks]] between them. The daemon sends every path '/'-separated,
// relative to the docs folder.

import type { Doc } from "../proto";

/** The group a doc is listed under: "" for agent-level (top of the folder), the
 * repository's folder name for `repos/<name>/…`, else its first folder. */
export function groupOf(path: string): string {
  const parts = path.split("/");
  if (parts.length === 1) return "";
  if (parts[0] === "repos" && parts.length > 2) return parts[1];
  return parts[0];
}

/** Agent-level first, then each repository's by name; newest first inside each,
 * as the daemon listed them. */
export function grouped(docs: Doc[]): [string, Doc[]][] {
  const groups = new Map<string, Doc[]>();
  for (const d of docs) {
    const g = groupOf(d.path);
    const list = groups.get(g);
    if (list) list.push(d);
    else groups.set(g, [d]);
  }
  return [...groups.entries()].sort(([a], [b]) => (a === "" ? -1 : b === "" ? 1 : a.localeCompare(b)));
}

/** A doc's text without its front matter, which the list already shows. */
export function body(text: string): string {
  const t = text.replace(/\r\n/g, "\n");
  if (!t.startsWith("---\n")) return t;
  const end = t.indexOf("\n---", 3);
  if (end < 0) return t;
  const after = t.indexOf("\n", end + 4);
  return after < 0 ? "" : t.slice(after + 1).replace(/^\n/, "");
}

/** A file's name without its folder and `.md`. */
function stem(path: string): string {
  return (path.split("/").pop() ?? path).replace(/\.md$/i, "");
}

/**
 * Where `[[target]]` goes: the key of the doc it names, by title, file name or
 * path under the folder (Obsidian's forms), or null if no listed doc goes by it.
 * Only docs the daemon listed resolve, so a wikilink can't reach anything else.
 */
export function resolver(docs: Doc[]): (target: string) => string | null {
  const by = new Map<string, string>();
  const add = (name: string, key: string) => {
    const k = name.trim().toLowerCase();
    if (k && !by.has(k)) by.set(k, key);
  };
  // Paths first: they're unique, so they win over a title another doc shares.
  for (const d of docs) if (d.key) add(d.path.replace(/\.md$/i, ""), d.key);
  for (const d of docs) if (d.key) add(stem(d.path), d.key);
  for (const d of docs) if (d.key) add(d.title, d.key);
  return (target) => {
    const name = target.split("#")[0].replace(/\.md$/i, "");
    return by.get(name.trim().toLowerCase()) ?? null;
  };
}

/** A key from the list, as a route can carry it: digits, or `p` and 16 hex. */
export function validKey(key: string): boolean {
  return /^(\d{1,10}|p[0-9a-f]{16})$/.test(key);
}

/** Longest quote the daemon keeps, in characters. */
const MAX_QUOTE = 500;

/**
 * A block's text as a comment quotes it, the way the daemon keeps it: hidden
 * characters out, on one line, cut at 500 characters. The same block gives the
 * same quote, so a comment finds its block again while the block reads the same.
 */
export function quoteOf(text: string): string {
  // Control characters and bidi overrides, as the daemon drops them.
  const shown = text.replace(/[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, " ");
  const line = shown.split(/\s+/).filter(Boolean).join(" ");
  const chars = [...line];
  return chars.length > MAX_QUOTE ? chars.slice(0, MAX_QUOTE).join("") + "…" : line;
}

/** How many comments each quote has, not counting closed ones. */
export function countByQuote(comments: { quote?: string; state: string }[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const c of comments) {
    if (!c.quote || c.state === "resolved") continue;
    out.set(c.quote, (out.get(c.quote) ?? 0) + 1);
  }
  return out;
}
