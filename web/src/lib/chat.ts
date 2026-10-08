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
    if (item.k === "tool") {
      run.push(item);
    } else {
      flush();
      out.push({ k: "item", item });
    }
  }
  flush();
  return out;
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
  else if (d.content) parts.push({ label: "content", text: d.content });
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
  /** Tables, kept as their text in a monospace block: readable, and scrolls sideways. */
  | { t: "pre"; text: string }
  | { t: "list"; ordered: boolean; items: string[] }
  | { t: "quote"; text: string }
  | { t: "hr" };

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
      out.push({ t: "pre", text: rows.join("\n") });
      continue;
    }
    const item = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      flush();
      const ordered = /\d/.test(item[1]);
      const items = [item[2]];
      for (i++; i < lines.length; i++) {
        const next = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i]);
        if (next && /\d/.test(next[1]) === ordered) items.push(next[2]);
        // An indented line continues the item above it.
        else if (/^\s{2,}\S/.test(lines[i]) && !/^\s*([-*+]|\d+[.)])\s/.test(lines[i]))
          items[items.length - 1] += " " + lines[i].trim();
        else break;
      }
      i--;
      out.push({ t: "list", ordered, items });
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
  | { t: "a"; v: string; href: string };

const INLINE = /`([^`\n]+)`|\*\*([^*\n]+)\*\*|(?<![\w*])\*([^*\s](?:[^*\n]*[^*\s])?)\*(?![\w*])|\[([^\]\n]+)\]\(([^)\s]+)\)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of src.matchAll(INLINE)) {
    if (m.index > last) out.push({ t: "text", v: src.slice(last, m.index) });
    if (m[1] !== undefined) out.push({ t: "code", v: m[1] });
    else if (m[2] !== undefined) out.push({ t: "b", v: m[2] });
    else if (m[3] !== undefined) out.push({ t: "i", v: m[3] });
    else if (m[4] !== undefined) {
      // Only web links open; a file path stays text.
      if (/^https?:\/\//.test(m[5])) out.push({ t: "a", v: m[4], href: m[5] });
      else out.push({ t: "text", v: m[4] });
    } else out.push({ t: "a", v: m[6], href: m[6] });
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
