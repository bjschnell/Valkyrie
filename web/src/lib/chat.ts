// The Chat view's logic, kept out of the components so it can be tested without a
// DOM: applying updates, grouping tool calls, the words on a card, and a small
// Markdown reader for what agents write.

import type { ChatItem, ChatUpdate, SessionId, ToolStatus } from "../proto";

export interface Chat {
  session: SessionId;
  items: ChatItem[];
  /** Older items exist that weren't loaded. */
  more: boolean;
  /** No transcript is known for the session. */
  missing: boolean;
}

/** Items kept; a long-running session drops its oldest. */
const MAX_ITEMS = 1500;

export function applyChat(chat: Chat | null, u: ChatUpdate): Chat {
  if (u.reset || !chat || chat.session !== u.session) {
    return { session: u.session, items: u.items, more: !!u.more, missing: !!u.missing || !!u.gone };
  }
  const items = chat.items.slice();
  const at = new Map(items.map((item, i) => [item.id, i]));
  for (const item of u.items) {
    const i = at.get(item.id);
    if (i === undefined) {
      at.set(item.id, items.length);
      items.push(item);
    } else {
      items[i] = item;
    }
  }
  const over = items.length - MAX_ITEMS;
  return over > 0
    ? { ...chat, items: items.slice(over), more: true, missing: false }
    : { ...chat, items, missing: false };
}

export type ToolItem = Extract<ChatItem, { k: "tool" }>;

/** What the list shows: messages one by one, and runs of tool calls as one group. */
export type Block = { k: "item"; item: ChatItem } | { k: "tools"; id: string; tools: ToolItem[] };

/** Runs shorter than this are shown as their cards. */
const GROUP_MIN = 3;

export function blocks(items: ChatItem[]): Block[] {
  const out: Block[] = [];
  let run: ToolItem[] = [];
  const flush = () => {
    if (run.length >= GROUP_MIN) out.push({ k: "tools", id: run[0].id, tools: run });
    else for (const item of run) out.push({ k: "item", item });
    run = [];
  };
  for (const item of items) {
    if (item.k === "tool" && !isQuestion(item)) {
      run.push(item);
    } else {
      flush();
      out.push({ k: "item", item });
    }
  }
  flush();
  return out;
}

/** The agent asking you something: shown in full, never folded into a run. */
export function isQuestion(card: ToolItem): boolean {
  return card.tool === "AskUserQuestion";
}

const EDITS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "Delete"]);
const READS = new Set(["Read", "View", "Grep", "Glob", "WebSearch", "WebFetch"]);

/** A group in one line: "Ran 4 commands, edited 2 files · 1 failed". */
export function groupSummary(tools: ToolItem[]): string {
  const ran = tools.filter((t) => t.tool === "Bash").length;
  const files = new Set(tools.filter((t) => EDITS.has(t.tool)).map((t) => t.detail.file ?? t.summary)).size;
  const read = tools.filter((t) => READS.has(t.tool)).length;
  const other = tools.length - ran - tools.filter((t) => EDITS.has(t.tool)).length - read;
  const parts: string[] = [];
  if (ran) parts.push(`ran ${plural(ran, "command")}`);
  if (files) parts.push(`edited ${plural(files, "file")}`);
  if (read) parts.push(`looked at ${read}`);
  if (other) parts.push(`${plural(other, "other call")}`);
  const failed = tools.filter((t) => t.status === "error").length;
  const denied = tools.filter((t) => t.status === "denied").length;
  let line = parts.join(", ");
  line = line.charAt(0).toUpperCase() + line.slice(1);
  if (failed) line += ` · ${failed} failed`;
  if (denied) line += ` · ${denied} denied`;
  return line;
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

export const TOOL_STATUS_LABEL: Record<ToolStatus, string> = {
  running: "running",
  ok: "done",
  error: "failed",
  denied: "denied",
};

/** What opening a card shows, in reading order; `null` if nothing beyond its line. */
export function cardBody(card: ToolItem): { label: string; text: string }[] | null {
  const d = card.detail;
  const parts: { label: string; text: string }[] = [];
  if (d.command) parts.push({ label: "command", text: `$ ${d.command}` });
  if (d.diff) parts.push({ label: "diff", text: d.diff });
  else if (d.content) parts.push({ label: isQuestion(card) ? "question" : "content", text: d.content });
  if (d.result) parts.push({ label: "result", text: d.result });
  return parts.length ? parts : null;
}

/** A file card's line, most telling first: `chat.rs`, `+1 -1`, then its directory. */
export function fileLine(card: ToolItem): { name: string; dir: string; added: number; removed: number; lines: number | null } | null {
  const file = card.detail.file;
  if (!file) return null;
  const slash = file.lastIndexOf("/");
  const edit = /\(\+(\d+)\/-(\d+)\)$/.exec(card.summary);
  const write = /\(\+(\d+) lines\)$/.exec(card.summary);
  return {
    name: file.slice(slash + 1),
    dir: file.slice(0, slash + 1),
    added: edit ? Number(edit[1]) : 0,
    removed: edit ? Number(edit[2]) : 0,
    lines: write ? Number(write[1]) : null,
  };
}

export function diffKind(line: string): "file" | "add" | "del" | "hunk" | "ctx" {
  if (line.startsWith("+++") || line.startsWith("---")) return "file";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  if (line.startsWith("@@")) return "hunk";
  return "ctx";
}

// -- Markdown, the parts agents use ----------------------------------------------

export type MdBlock =
  | { t: "p"; text: string }
  | { t: "h"; level: number; text: string }
  | { t: "code"; lang: string; text: string }
  /** A table: a header row, the `|---|:-:|` row under it, then its rows. Cells are inline markdown. */
  | { t: "table"; head: string[]; align: Align[]; rows: string[][] }
  /** Pipe lines that aren't a table (no separator row), kept as their text in a monospace block. */
  | { t: "pre"; text: string }
  /** `start` is an ordered list's first number, so steps split by a heading still count on.
   * `checks`: a task list's boxes (`- [x] done`), null for an item without one; shown, never ticked. */
  | { t: "list"; ordered: boolean; items: string[]; start?: number; checks?: (boolean | null)[] }
  | { t: "quote"; text: string }
  | { t: "hr" };

export type Align = "left" | "center" | "right" | null;

const LIST_ITEM = /^\s*([-*+]|\d+[.)])\s+(.*)$/;

/** A table row's cells: `| a | b |` or `a | b`, with `\|` kept in a cell. */
function cells(line: string): string[] {
  let row = line.trim();
  if (row.startsWith("|")) row = row.slice(1);
  if (row.endsWith("|") && !row.endsWith("\\|")) row = row.slice(0, -1);
  return row.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, "|"));
}

/** The `|---|:--:|` row under a table's header, as each column's alignment; null if it isn't one. */
function separator(line: string | undefined): Align[] | null {
  if (line === undefined || !line.includes("-")) return null;
  const parts = cells(line);
  if (!parts.every((c) => /^:?-+:?$/.test(c))) return null;
  return parts.map((c) => (c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : c.startsWith(":") ? "left" : null));
}

/** A task list item's box, and the text after it. */
function task(item: string): [boolean | null, string] {
  const m = /^\[([ xX])\]\s+(.*)$/s.exec(item);
  return m ? [m[1] !== " ", m[2]] : [null, item];
}

/** A line that starts a block of its own, so it can't continue a list item. */
function startsBlock(line: string): boolean {
  return (
    /^\s*(```|~~~)/.test(line) ||
    /^#{1,6}\s/.test(line) ||
    /^\s*([-*_])(\s*\1){2,}\s*$/.test(line) ||
    line.trimStart().startsWith("|") ||
    line.startsWith(">")
  );
}

export function parseMarkdown(src: string): MdBlock[] {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const out: MdBlock[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push({ t: "p", text: para.join("\n") });
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const fence = /^\s*(```+|~~~+)\s*([\w+-]*)/.exec(line);
    if (fence) {
      flush();
      const body: string[] = [];
      for (i++; i < lines.length && !lines[i].trimStart().startsWith(fence[1]); i++) body.push(lines[i]);
      out.push({ t: "code", lang: fence[2], text: body.join("\n") });
      continue;
    }
    if (!line.trim()) {
      flush();
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      out.push({ t: "h", level: heading[1].length, text: heading[2] });
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      out.push({ t: "hr" });
      continue;
    }
    if (line.trimStart().startsWith("|")) {
      flush();
      const rows: string[] = [];
      for (; i < lines.length && lines[i].trimStart().startsWith("|"); i++) rows.push(lines[i].trim());
      i--;
      const align = separator(rows[1]);
      if (align) {
        const width = align.length;
        const fit = (r: string[]) => Array.from({ length: width }, (_, k) => r[k] ?? "");
        out.push({ t: "table", head: fit(cells(rows[0])), align, rows: rows.slice(2).map((r) => fit(cells(r))) });
      } else out.push({ t: "pre", text: rows.join("\n") });
      continue;
    }
    const item = LIST_ITEM.exec(line);
    if (item) {
      flush();
      const ordered = /\d/.test(item[1]);
      const items = [item[2]];
      const sameList = (l: string | undefined) => {
        const m = l === undefined ? null : LIST_ITEM.exec(l);
        return !!m && /\d/.test(m[1]) === ordered;
      };
      for (i++; i < lines.length; i++) {
        const next = lines[i];
        if (LIST_ITEM.test(next)) {
          if (!sameList(next)) break;
          items.push(LIST_ITEM.exec(next)![2]);
        } else if (!next.trim()) {
          // A blank line ends the list, unless another item or an indented
          // paragraph of this one comes after it ("1. step", blank, "2. step").
          let j = i + 1;
          while (j < lines.length && !lines[j].trim()) j++;
          if (sameList(lines[j])) i = j - 1;
          else if (j < lines.length && /^\s{2,}\S/.test(lines[j])) {
            items[items.length - 1] += "\n\n" + lines[j].trim();
            i = j;
          } else break;
        } else if (/^\s{2,}\S/.test(next) || !startsBlock(next)) {
          // An indented line, or a plain one right under the item (a command
          // below a step), continues the item above it.
          items[items.length - 1] += "\n" + next.trim();
        } else break;
      }
      i--;
      const list: MdBlock & { t: "list" } = ordered
        ? { t: "list", ordered, items, start: parseInt(item[1], 10) }
        : { t: "list", ordered, items };
      const boxes = items.map(task);
      if (boxes.some(([c]) => c !== null)) {
        list.items = boxes.map(([, text]) => text);
        list.checks = boxes.map(([c]) => c);
      }
      out.push(list);
      continue;
    }
    if (line.startsWith(">")) {
      flush();
      const quote: string[] = [];
      for (; i < lines.length && lines[i].startsWith(">"); i++) quote.push(lines[i].replace(/^>\s?/, ""));
      i--;
      out.push({ t: "quote", text: quote.join("\n") });
      continue;
    }
    para.push(line);
  }
  flush();
  return out;
}

export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "b"; v: string }
  | { t: "i"; v: string }
  | { t: "a"; v: string; href: string }
  /** `[[target]]` or `[[target|label]]`: a link to another doc, if one goes by that name. */
  | { t: "wiki"; v: string; target: string };

// No class here takes a `[`, so a long run of them can't make the match re-scan
// from every position (a doc can be one 2 MB line).
const INLINE = /\[\[([^[\]|\n]+)(?:\|([^[\]\n]+))?\]\]|`([^`\n]+)`|\*\*([^*\n]+)\*\*|(?<![\w*])\*([^*\s](?:[^*\n]*[^*\s])?)\*(?![\w*])|\[([^[\]\n]+)\]\(([^)\s]+)\)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of src.matchAll(INLINE)) {
    if (m.index > last) out.push({ t: "text", v: src.slice(last, m.index) });
    if (m[1] !== undefined) out.push({ t: "wiki", v: (m[2] ?? m[1]).trim(), target: m[1].trim() });
    else if (m[3] !== undefined) out.push({ t: "code", v: m[3] });
    else if (m[4] !== undefined) out.push({ t: "b", v: m[4] });
    else if (m[5] !== undefined) out.push({ t: "i", v: m[5] });
    else if (m[6] !== undefined) {
      // Only web links open; a file path stays text.
      if (/^https?:\/\//i.test(m[7])) out.push({ t: "a", v: m[6], href: m[7] });
      else out.push({ t: "text", v: m[6] });
    } else out.push({ t: "a", v: m[8], href: m[8] });
    last = m.index + m[0].length;
  }
  if (last < src.length) out.push({ t: "text", v: src.slice(last) });
  return out;
}

/** "14:02" today, "Mon 14:02" this week, else the date. */
export function clock(at: string | undefined, now = Date.now()): string {
  if (!at) return "";
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return "";
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const days = (now - d.getTime()) / 86_400_000;
  if (days < 1 && new Date(now).getDate() === d.getDate()) return time;
  if (days < 6) return `${d.toLocaleDateString([], { weekday: "short" })} ${time}`;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}
