// The choices a prompt on screen offers (Claude Code's permission dialog and trust
// check, Codex's approvals, most TUIs' pickers), so the phone can show buttons.
// Numbered options are answered with their number, which both agents take;
// unnumbered ones by moving the program's cursor (`❯`) there and pressing Enter.

export interface Choice {
  label: string;
  /** The program's cursor is on it. */
  selected: boolean;
  /** Its number, when the prompt numbers its options. */
  n?: number;
  /** Arrow presses from the cursor to it (negative is up), for unnumbered ones. */
  moves?: number;
}

const CURSOR = "[❯›>▶→]";
const NUMBERED = new RegExp(`^(\\s*)(${CURSOR})?\\s*(\\d)[.)]\\s+(.+?)\\s*$`, "u");
const POINTED = new RegExp(`^(\\s*)${CURSOR}\\s+(\\S.*?)\\s*$`, "u");

/**
 * The prompt in the screen's bottom lines: at least two choices, one under the
 * program's cursor. Without a cursor it's a numbered list in some output, not a
 * question.
 */
export function findChoices(lines: string[], lookback = 25): Choice[] {
  const tail = lines.slice(Math.max(0, lines.length - lookback));
  const numbered = findNumbered(tail);
  const found = numbered.length >= 2 ? numbered : findPointed(tail);
  return found.some((c) => c.selected) ? found : [];
}

/**
 * The last run of options numbered 1, 2, 3…. Indented lines between two options
 * continue the label above them (a wrapped option); a blank line or two may sit
 * between options, more ends the run.
 */
function findNumbered(lines: string[]): Choice[] {
  let best: Choice[] = [];
  let run: Choice[] = [];
  let gap = 0;
  const close = () => {
    if (run.length >= 2) best = run;
    run = [];
    gap = 0;
  };
  for (const line of lines) {
    const m = NUMBERED.exec(line);
    if (m) {
      const n = Number(m[3]);
      if (n !== run.length + 1) close();
      if (n === run.length + 1) {
        run.push({ n, label: clean(m[4]), selected: Boolean(m[2]) });
        gap = 0;
      }
      continue;
    }
    if (!run.length) continue;
    const text = line.trim();
    if (text && gap === 0 && /^\s{3,}/.test(line)) {
      // A wrapped label goes on under its option.
      const last = run[run.length - 1];
      last.label = clean(`${last.label} ${text}`);
    } else if (++gap > 2) {
      close();
    }
  }
  close();
  return best;
}

/** Unnumbered options: the cursor's line and its neighbors aligned with it. */
function findPointed(lines: string[]): Choice[] {
  for (let i = lines.length - 1; i >= 0; i--) {
    const m = POINTED.exec(lines[i]);
    if (!m) continue;
    // The option text starts at the same column on every line.
    const column = lines[i].indexOf(m[2]);
    const option = (j: number) => {
      const line = lines[j];
      if (line === undefined || line.trim() === "") return null;
      const lead = line.length - line.trimStart().length;
      return lead === column ? line.trim() : null;
    };
    const above: string[] = [];
    for (let j = i - 1; option(j); j--) above.unshift(option(j)!);
    const below: string[] = [];
    for (let j = i + 1; option(j); j++) below.push(option(j)!);
    const labels = [...above, m[2], ...below];
    if (labels.length < 2) return [];
    return labels.map((label, k) => ({
      label: clean(label),
      selected: k === above.length,
      moves: k - above.length,
    }));
  }
  return [];
}

/** Box-drawing borders and key hints (`(esc)`, `(y)`) are not part of the label. */
function clean(label: string): string {
  return label
    .replace(/[│┃║]+\s*$/u, "")
    .replace(/\s+\((?:esc|y|n|a|p|shift\+tab|tab)\)$/iu, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/** A refusal is styled as one. */
export function isNegative(label: string): boolean {
  return /^(no\b|deny|reject|cancel|abort|don'?t)/i.test(label);
}

/**
 * The prompt's plain yes, if its first choice is one: what "allow all" picks. Never
 * a broader grant ("Yes, and don't ask again", "always"), which stays a choice you
 * make on the prompt itself.
 */
export function plainYes(choices: Choice[]): Choice | null {
  const first = choices[0];
  if (!first) return null;
  const yes = /^(yes|allow|approve|proceed)\b/i.test(first.label);
  const broader = /(always|don'?t ask|for this session|all\b|every)/i.test(first.label);
  return yes && !broader ? first : null;
}

const GENERIC = /^(do you want|would you like|press enter|select an option|esc to|enter to|tip:)/i;
const BORDER = /^[\s│┃║╭╮╰╯─━═┌┐└┘]+|[\s│┃║╭╮╰╯─━═┌┐└┘]+$/gu;

/**
 * What a prompt on screen is about, in a line: the paragraph just above its
 * choices (the command, the file), past generic lines like "Do you want to
 * proceed?". For a prompt known only from the screen, whose summary says nothing.
 */
export function promptContext(lines: string[], lookback = 25): string | null {
  const tail = lines.slice(Math.max(0, lines.length - lookback));
  let first = -1;
  for (let i = tail.length - 1; i >= 0; i--) {
    const numbered = NUMBERED.exec(tail[i]);
    if ((numbered && numbered[3] === "1") || (first < 0 && POINTED.test(tail[i]))) first = i;
    if (numbered && numbered[3] === "1") break;
  }
  if (first < 0) return null;
  const para: string[] = [];
  for (let i = first - 1; i >= 0 && para.length < 3; i--) {
    const text = tail[i].replace(BORDER, "").trim();
    if (!text) {
      if (para.length) break;
      continue;
    }
    if (GENERIC.test(text)) {
      if (para.length) break;
      continue;
    }
    if (NUMBERED.test(tail[i]) || POINTED.test(tail[i])) break;
    para.unshift(text.replace(/^\$\s+/, ""));
  }
  if (!para.length) return null;
  const line = para.join(" — ");
  return line.length > 140 ? `${line.slice(0, 139)}…` : line;
}

/** A summary that says nothing about what is being asked. */
export function weakSummary(summary: string | null | undefined): boolean {
  return !summary || /^(bell|screen|needs input)$/i.test(summary.trim());
}
