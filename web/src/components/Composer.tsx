import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { HASH } from "../lib/routes";
import { base, itemRef, priority, startAgent, MODEL_CHOICES } from "../lib/backlog";
import {
  command,
  findItems,
  guessRepo,
  intent,
  lastRan,
  ranLabel,
  readyItems,
  slug,
  supervisorLine,
  type Found,
  type Ran,
} from "../lib/compose";
import { repoChoices } from "../lib/repos";
import { age } from "../lib/time";
import { addItem, launchItem, spawn, supervisorLog, supervisorWatch, toast, useApp } from "../store";
import { useCached } from "../cache";
import { useRepos } from "./Repos";
import { MAC } from "./Terminal";
import type { Item } from "../proto";
import { useAttention } from "./Attention";

const DRAFT_KEY = "valk.compose.draft";
const REPO_KEY = "valk.compose.repo";
const RAN_KEY = "valk.compose.ran";
/** Says the draft changed from outside the box (Start agents gives it back). */
const DRAFT_EVENT = "valk-compose-draft";

function read<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}

function write(key: string, v: unknown): void {
  try {
    if (v === null || v === "") localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* kept for this visit only */
  }
}

/** The composer's draft as it was last typed: what Start agents opens with. */
export const composerDraft = () => read<string>(DRAFT_KEY, "");

/** Replaces the composer's draft, drawn or not. */
export function setComposerDraft(text: string): void {
  write(DRAFT_KEY, text);
  window.dispatchEvent(new CustomEvent(DRAFT_EVENT, { detail: text }));
}

/** The repo started in last, and what each repo last ran from here (decision #46). */
export const lastRepo = () => read<string | null>(REPO_KEY, null);
export const rememberedRan = () => read<Record<string, Ran>>(RAN_KEY, {});

/** Remembers a start in `repo` on `ran`, for the next guess. */
export function rememberStart(repo: string, ran: Ran): void {
  write(REPO_KEY, repo);
  write(RAN_KEY, { ...rememberedRan(), [repo]: ran });
}

// `n` or `/` asks for the box before it may be drawn (the Inbox moves to `#/new` first).
let wanted = false;

/** Puts the keyboard in the home composer, now or as soon as it's drawn. */
export function focusComposer(): void {
  const el = document.querySelector<HTMLTextAreaElement>(".composer-text");
  if (el) {
    el.focus();
    wanted = false;
  } else wanted = true;
}

/** The launch offers' items: drawn in Needs you already, so not again here. */
export function useOffered(): Set<number> {
  const launches = useApp((s) => s.launches);
  return useMemo(() => new Set(launches.map((l) => l.item)), [launches]);
}

/**
 * The home composer (backlog #275): what the Inbox shows when nothing is picked.
 * One box, never focused until asked (`n`, `/`, a click), so the Inbox's keys stay
 * yours. A line and Enter starts a session in its own worktree; ⌘/Ctrl+Enter
 * files it to the backlog; `#` finds an item; several lines go to Start agents.
 * `phone` is the slim one pinned over the tab bar.
 */
export function Composer({ phone = false }: { phone?: boolean }) {
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const status = useApp((s) => s.status);
  const { list } = useRepos();
  const repos = useMemo(() => (list?.repos ?? []).map((r) => r.path), [list]);

  const [text, setText] = useState(() => read<string>(DRAFT_KEY, ""));
  // A repo or agent picked on the chips holds until the box is cleared.
  const [pickedRepo, setPickedRepo] = useState<string | null>(null);
  const [pickedRan, setPickedRan] = useState<Ran | null>(null);
  const [busy, setBusy] = useState(false);
  const [hi, setHi] = useState(0);
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => write(DRAFT_KEY, text), [text]);
  useEffect(() => {
    if (text === "") {
      setPickedRepo(null);
      setPickedRan(null);
    }
  }, [text]);
  useEffect(() => {
    if (wanted) focusComposer();
    const given = (e: Event) => setText((e as CustomEvent<string>).detail);
    window.addEventListener(DRAFT_EVENT, given);
    return () => window.removeEventListener(DRAFT_EVENT, given);
  }, []);
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, phone ? 140 : 240)}px`;
  }, [text, phone]);

  const what = intent(text);
  const repo = pickedRepo ?? guessRepo(what.kind === "task" ? what.text : text, repos, lastRepo());
  const ran = pickedRan ?? lastRan(repo, rememberedRan(), items, sessions);
  const found: Found[] = what.kind === "find" ? findItems(what.query, items, sessions, boot) : [];
  useEffect(() => setHi(0), [what.kind === "find" ? what.query : null]);

  const done = () => {
    setText("");
    area.current?.blur();
  };

  const run = async (f: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };

  const start = () =>
    run(async () => {
      if (what.kind !== "task" || !repo) return;
      try {
        const id = await spawn(command(ran, what.text), repo, slug(what.text), "");
        rememberStart(repo, ran);
        done();
        navigator.vibrate?.(10);
        location.hash = `#/s/${id}`;
      } catch (e) {
        toast(`Couldn't start: ${(e as Error).message}`);
      }
    });

  // Typed by you, so it's a real item, not a proposal (decision #14).
  const file = () =>
    run(async () => {
      const body = text.trim();
      if (!body) return;
      try {
        await addItem({ title: "", body, kind: "feature", cwd: repo, attachments: [] });
        // The keyboard stays: filing several in a row is the usual way.
        setText("");
        toast(`Filed to the backlog${repo ? ` in ${base(repo)}` : ""}`);
      } catch (e) {
        toast(`Couldn't file it: ${(e as Error).message}`);
      }
    });

  // Several lines: Start agents (#282), a session per line, checked there first.
  // The draft stays until they start, so Esc there comes back to it.
  const batch = () => {
    area.current?.blur();
    location.hash = HASH.start;
  };

  const pick = (f: Found | undefined) => {
    if (!f) return;
    if (f.pick === "open" && f.session) {
      done();
      location.hash = `#/s/${f.session.id}`;
    } else if (f.pick === "start") {
      void run(async () => {
        try {
          const id = await launchItem(f.item.id, startAgent(f.item, ran.agent));
          done();
          location.hash = `#/s/${id}`;
        } catch (e) {
          toast(`Couldn't start: ${(e as Error).message}`);
        }
      });
    }
  };

  const onKey = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.currentTarget.blur();
      return;
    }
    if (what.kind === "find" && found.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setHi((h) => (h + (e.key === "ArrowDown" ? 1 : found.length - 1)) % found.length);
      return;
    }
    if (e.key !== "Enter" || e.shiftKey) return;
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) return void file();
    if (what.kind === "task") void start();
    else if (what.kind === "find") pick(found[hi]);
    else if (what.kind === "batch") batch();
  };

  const offline = status !== "open";
  const canStart = !busy && !offline && ((what.kind === "task" && !!repo) || what.kind === "batch");
  const primary = what.kind === "batch" ? `Review and start ${what.lines.length}` : "Start";
  const go = () => (what.kind === "batch" ? batch() : void start());
  const mod = MAC ? "⌘" : "Ctrl";

  const chips = (
    <div className="composer-chips">
      <RepoChip repo={repo} list={list} onPick={setPickedRepo} />
      <AgentChip ran={ran} onPick={setPickedRan} />
    </div>
  );

  const findList = what.kind === "find" && (
    <div className="composer-found" role="listbox" aria-label="Backlog items">
      {found.length === 0 && <div className="composer-found-none">No open item matches</div>}
      {found.map((f, i) => (
        <button
          key={f.item.id}
          role="option"
          aria-selected={i === hi}
          aria-disabled={f.pick === "blocked" || undefined}
          className={`composer-hit ${i === hi ? "hi" : ""} ${f.pick === "blocked" ? "blocked" : ""}`}
          onMouseEnter={() => setHi(i)}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => pick(f)}
        >
          <span className="composer-ref">{itemRef(f.item)}</span>
          <span className="composer-hit-title">{f.item.title || f.item.body.split("\n")[0]}</span>
          {f.pick === "open" && f.session && (
            <span className={`composer-hit-state state-text-${f.session.status.state}`}>{f.session.status.state === "working" ? "running" : "has a session"}</span>
          )}
          {f.pick === "start" && <span className="composer-hit-state">ready · {startAgent(f.item, ran.agent)}</span>}
          {f.why && <span className="composer-hit-state">{f.why}</span>}
          {f.pick !== "blocked" && <span className="composer-hit-key">↵ {f.pick}</span>}
        </button>
      ))}
    </div>
  );

  const box = (
    <textarea
      ref={area}
      className="composer-text"
      rows={1}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onKeyDown={onKey}
      placeholder={phone ? "Start an agent…" : "Describe a task, or type # for a backlog item"}
      aria-label="What's next"
      enterKeyHint="go"
      spellCheck
    />
  );

  if (phone) {
    return (
      <div className="home-composer composer-phone">
        {findList}
        {chips}
        <div className="composer-pill">
          {box}
          <button className="composer-go" aria-label={primary} disabled={!canStart} onClick={go}>
            <Arrow />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="home-composer">
      <div className={`composer-box ${what.kind === "find" ? "finding" : ""}`}>
        <div className="composer-line">
          {box}
          {!text && <kbd className="composer-hint">n</kbd>}
        </div>
        {what.kind !== "find" && (
          <div className="composer-row">
            {what.kind === "batch" ? (
              <span className="composer-batch">
                {what.lines.length} tasks · an agent each
              </span>
            ) : (
              chips
            )}
            <span className="composer-gap" />
            {what.kind !== "empty" && (
              <button className="ghost composer-file" disabled={busy || offline} onClick={() => void file()} title={`${mod}+Enter`}>
                File to backlog <kbd>{mod}↵</kbd>
              </button>
            )}
            <button className={what.kind === "empty" ? "composer-go" : "primary composer-start"} aria-label={primary} disabled={!canStart} onClick={go}>
              {what.kind === "empty" ? (
                <Arrow />
              ) : (
                <>
                  {primary} <kbd>↵</kbd>
                </>
              )}
            </button>
          </div>
        )}
      </div>
      {findList}
    </div>
  );
}

function Arrow() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 19V5M5 12l7-7 7 7" />
    </svg>
  );
}

export function RepoChip({ repo, list, onPick }: { repo: string | null; list: ReturnType<typeof useRepos>["list"]; onPick: (r: string) => void }) {
  const choices = repoChoices(list, repo);
  if (choices.length === 0) return <span className="composer-chip none">no repo</span>;
  return (
    <label className="composer-chip">
      <span className="sr-only">Repository</span>
      <select value={repo ?? ""} onChange={(e) => onPick(e.target.value)}>
        {choices.map((c) => (
          <option key={c.repo} value={c.repo}>
            {c.label}
          </option>
        ))}
      </select>
      <span aria-hidden="true">{repo ? base(repo) : "repo"} ▾</span>
    </label>
  );
}

const ranKey = (r: Ran) => `${r.agent}|${r.model ?? ""}`;

export function AgentChip({ ran, onPick }: { ran: Ran; onPick: (r: Ran) => void }) {
  const options: Ran[] = [
    { agent: "claude" },
    ...MODEL_CHOICES.claude.map((model) => ({ agent: "claude" as const, model })),
    { agent: "codex" },
  ];
  if (!options.some((o) => ranKey(o) === ranKey(ran))) options.push(ran);
  return (
    <label className="composer-chip">
      <span className="sr-only">Agent and model</span>
      <select
        value={ranKey(ran)}
        onChange={(e) => {
          const [agent, model] = e.target.value.split("|");
          onPick(model ? { agent: agent as Ran["agent"], model } : { agent: agent as Ran["agent"] });
        }}
      >
        {options.map((o) => (
          <option key={ranKey(o)} value={ranKey(o)}>
            {ranLabel(o)}
          </option>
        ))}
      </select>
      <span aria-hidden="true">{ranLabel(ran)} ▾</span>
    </label>
  );
}

/**
 * The top three items ready to start, each with Start (decision #46: on the agent
 * it last ran). j/k move through them, Enter starts, Esc lets go (decision #77, rule 7).
 */
export function ReadyList({ compact = false }: { compact?: boolean }) {
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const offered = useOffered();
  const ready = readyItems(items, sessions, boot, offered);
  const all = readyItems(items, sessions, boot, offered, Infinity).length;
  const [busy, setBusy] = useState<number | null>(null);
  if (ready.length === 0) return null;

  const start = async (item: Item) => {
    if (busy !== null) return;
    setBusy(item.id);
    try {
      const id = await launchItem(item.id, startAgent(item, null));
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Couldn't start: ${(e as Error).message}`);
    } finally {
      setBusy(null);
    }
  };

  const onKey = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const rows = [...e.currentTarget.querySelectorAll<HTMLButtonElement>(".ready-start")];
    const at = rows.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "j" || e.key === "k") {
      rows[Math.max(0, Math.min(rows.length - 1, at + (e.key === "j" ? 1 : -1)))]?.focus();
    } else if (e.key === "Escape") {
      (document.activeElement as HTMLElement | null)?.blur();
    } else if (e.key !== "Enter") return;
    // The Inbox's own j/k/Enter/Esc are for its list, not this one.
    e.stopPropagation();
    if (e.key !== "Enter") e.preventDefault();
  };

  return (
    <section className={`ready ${compact ? "compact" : ""}`} aria-label="Ready in the backlog" onKeyDown={onKey}>
      <div className="ready-head">
        <h2 className="ready-title">Ready in the backlog</h2>
        {!compact && (
          <a className="ready-all" href={HASH.backlog}>
            all {all} in Backlog
          </a>
        )}
      </div>
      {ready.map((item) => (
        <div key={item.id} className="ready-row">
          <span className="ready-ref">{itemRef(item)}</span>
          <span className="ready-name">{item.title || item.body.split("\n")[0]}</span>
          {!compact && (
            <span className="ready-meta">
              {item.kind} · {priority(item)}
            </span>
          )}
          <button className="ghost ready-start" disabled={busy !== null} onClick={() => void start(item)}>
            {busy === item.id ? "Starting…" : "Start"}
          </button>
        </div>
      ))}
    </section>
  );
}

/** The supervisor in one line at the foot of the pane; the rest is in History. */
export function SupervisorLine() {
  const now = useApp((s) => s.now);
  const status = useApp((s) => s.status);
  const { data: watch, reload: reWatch } = useCached("supervisor-watch", supervisorWatch);
  const { data: log, reload: reLog } = useCached("supervisor-log", supervisorLog);
  useEffect(() => {
    if (status !== "open") return;
    const every = setInterval(() => {
      void reWatch();
      void reLog();
    }, 60_000);
    return () => clearInterval(every);
  }, [status, reWatch, reLog]);
  if (!watch && !log) return null;
  const on = (watch?.on ?? true) && (log?.on ?? true);
  const last = log?.entries.length ? Math.max(...log.entries.map((e) => e.at)) : null;
  return (
    <div className="supervisor-line">
      <span className={`supervisor-dot ${on ? "on" : ""}`} aria-hidden="true" />
      {supervisorLine(watch?.prs.length ?? null, on, last, now, age)} ·{" "}
      <a href={HASH.history}>History</a>
    </div>
  );
}

/** The pane beside the Inbox when nothing is picked: the composer, what's ready, the supervisor's line. */
export function HomePane() {
  const needs = useAttention().length;
  return (
    <div className="home-pane">
      <div className="home-center">
        <div className="home-head">
          <h1 className="home-title">What's next?</h1>
          <p className="home-sub">{needs ? "Start an agent while those wait, or pick up the backlog." : "Nothing needs you. Start an agent, or pick up the backlog."}</p>
        </div>
        <Composer />
        <ReadyList />
      </div>
      <SupervisorLine />
    </div>
  );
}
