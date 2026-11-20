// Where the four sections live (#249): Inbox, Backlog, Memory, History. Docs and
// Wiki are tabs in Memory; Summary, Search and Supervisor are tabs in History.
// The pages they used to be at redirect, so old links and bookmarks still land.

export const HASH = {
  inbox: "#/",
  backlog: "#/backlog",
  memory: "#/memory",
  skills: "#/memory/skills",
  policies: "#/memory/policies",
  habits: "#/memory/habits",
  notes: "#/memory/notes",
  docs: "#/memory/docs",
  wiki: "#/memory/wiki",
  history: "#/history",
  summary: "#/history/summary",
  search: "#/history/search",
  supervisor: "#/history/supervisor",
  settings: "#/settings",
} as const;

/** Memory's shelves (#253), each a tab. `#/memory` is Decisions; a phone lands
 * there on an overview instead, and reaches Decisions at `#/memory/decisions`. */
export type MemoryShelf = "decisions" | "skills" | "policies" | "habits" | "notes";
export const SHELF_HASH: Record<MemoryShelf, string> = {
  decisions: `${HASH.memory}/decisions`,
  skills: HASH.skills,
  policies: HASH.policies,
  habits: HASH.habits,
  notes: HASH.notes,
};

/** The shelf a Memory address is, or undefined for none: null is `#/memory`. */
export function memoryShelf(hash: string): MemoryShelf | null | undefined {
  if (hash === HASH.memory) return null;
  const m = /^#\/memory\/(decisions|skills|policies|habits|notes)$/.exec(hash);
  return m ? (m[1] as MemoryShelf) : undefined;
}

/** One doc in Memory's Docs tab, by its key (never a path). */
export const docHash = (key: string) => `${HASH.docs}/${encodeURIComponent(key)}`;

const MOVED: Record<string, string> = {
  "#/decisions": HASH.memory,
  "#/docs": HASH.docs,
  "#/wiki": HASH.wiki,
  "#/summary": HASH.summary,
  "#/digest": HASH.summary,
  "#/search": HASH.search,
  "#/supervisor": HASH.supervisor,
};

/** Where an old page's hash lives now, or null for one that hasn't moved. */
export function moved(hash: string): string | null {
  if (MOVED[hash]) return MOVED[hash];
  const doc = /^#\/docs\/([^/]+)$/.exec(hash);
  return doc ? `${HASH.docs}/${doc[1]}` : null;
}

/** The four sections, by the second key of their `g` chord. */
export const SECTION_KEYS: Record<string, string> = {
  i: HASH.inbox,
  b: HASH.backlog,
  m: HASH.memory,
  h: HASH.history,
};
