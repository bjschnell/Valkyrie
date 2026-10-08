// A session's screen in the browser: the daemon's styled rows, kept current from
// `screen` pushes (the same full-then-diff updates the TUI applies), and turned
// into lines to show, either as the grid or reflowed to the phone's width.

import {
  BOLD,
  DIM,
  HIDDEN,
  INVERSE,
  ITALIC,
  STRIKEOUT,
  UNDERLINE,
  type Color,
  type Modes,
  type Row,
  type ScreenUpdate,
  type Style,
} from "../proto";

export interface Screen {
  cols: number;
  rows: Row[];
  cursor: { x: number; y: number; visible: boolean };
  modes: Modes;
  title: string | null;
}

export function apply(prev: Screen | null, update: ScreenUpdate): Screen {
  const height = update.size.rows;
  let rows: Row[];
  if (!prev || update.full || prev.rows.length !== height) {
    rows = Array.from({ length: height }, (_, y) => ({ y, spans: [] }));
  } else {
    rows = prev.rows.slice();
  }
  for (const row of update.rows) {
    if (row.y < rows.length) rows[row.y] = row;
  }
  return {
    cols: update.size.cols,
    rows,
    cursor: update.cursor,
    modes: update.modes,
    title: update.title,
  };
}

/** Terminal columns a character takes: 2 for wide CJK and most emoji. */
export function charWidth(cp: number): number {
  if (cp < 0x1100) return 1;
  if (
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe4f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) ||
    (cp >= 0x1f900 && cp <= 0x1f9ff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  )
    return 2;
  return 1;
}

export function textWidth(text: string): number {
  let w = 0;
  for (const ch of text) w += charWidth(ch.codePointAt(0)!);
  return w;
}

/** A run of text with one style, gaps between the daemon's spans filled. */
export interface Run {
  text: string;
  style?: Style;
}

/** A row as runs from column 0, spaces filling the gaps between spans. */
export function rowRuns(row: Row): Run[] {
  const runs: Run[] = [];
  let col = 0;
  for (const span of [...row.spans].sort((a, b) => a.x - b.x)) {
    if (span.x > col) runs.push({ text: " ".repeat(span.x - col) });
    runs.push({ text: span.text, style: span.style });
    col = Math.max(col, span.x) + textWidth(span.text);
  }
  return runs;
}

export function rowText(row: Row): string {
  return rowRuns(row)
    .map((r) => r.text)
    .join("");
}

/** The screen as plain text lines, trailing blank lines dropped. */
export function screenText(screen: Screen): string[] {
  const lines = screen.rows.map((r) => rowText(r).trimEnd());
  while (lines.length && !lines[lines.length - 1]) lines.pop();
  return lines;
}

/**
 * Logical lines: rows the program soft-wrapped joined back up, so a phone can wrap
 * them at its own width. Trailing blank lines are dropped; runs keep their styles.
 */
export function reflow(screen: Screen): Run[][] {
  const lines: Run[][] = [];
  let current: Run[] = [];
  for (const row of screen.rows) {
    const runs = rowRuns(row);
    if (!row.wrapped) trimEnd(runs);
    current.push(...runs);
    if (!row.wrapped) {
      lines.push(current);
      current = [];
    }
  }
  if (current.length) lines.push(current);
  while (lines.length && lines[lines.length - 1].every((r) => !r.text.trim())) lines.pop();
  return lines;
}

function trimEnd(runs: Run[]): void {
  while (runs.length) {
    const last = runs[runs.length - 1];
    const trimmed = last.text.replace(/\s+$/, "");
    if (trimmed || (last.style?.bg && last.style.bg !== "default")) {
      last.text = trimmed || last.text;
      return;
    }
    runs.pop();
  }
}

/** The Dracula ANSI colors (the TUI's default theme), then the xterm 256 cube. */
const ANSI = [
  "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
  "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
];

export function cssColor(c: Color | undefined): string | undefined {
  if (!c || c === "default") return undefined;
  if ("rgb" in c) return `rgb(${c.rgb[0]},${c.rgb[1]},${c.rgb[2]})`;
  const i = c.indexed;
  if (i < 16) return ANSI[i];
  if (i < 232) {
    const n = i - 16;
    const level = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `rgb(${level(Math.floor(n / 36))},${level(Math.floor(n / 6) % 6)},${level(n % 6)})`;
  }
  const g = 8 + (i - 232) * 10;
  return `rgb(${g},${g},${g})`;
}

export function cssStyle(style: Style | undefined): React.CSSProperties | undefined {
  if (!style) return undefined;
  const flags = style.flags ?? 0;
  let fg = cssColor(style.fg);
  let bg = cssColor(style.bg);
  if (flags & INVERSE) {
    [fg, bg] = [bg ?? "var(--term-bg)", fg ?? "var(--term-fg)"];
  }
  const css: React.CSSProperties = {};
  if (fg) css.color = fg;
  if (bg) css.background = bg;
  if (flags & BOLD) css.fontWeight = 700;
  if (flags & ITALIC) css.fontStyle = "italic";
  if (flags & DIM) css.opacity = 0.6;
  if (flags & HIDDEN) css.visibility = "hidden";
  const lines = [flags & UNDERLINE && "underline", flags & STRIKEOUT && "line-through"].filter(
    Boolean,
  );
  if (lines.length) css.textDecoration = lines.join(" ");
  return css;
}
