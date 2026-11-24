import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { HASH } from "../lib/routes";
import { base, itemRef } from "../lib/backlog";
import { draft, itemRow, lines, pickable, rowsFrom, spoken, summary, taskRow, type Context, type Row } from "../lib/batch";
import { command, lastRan, slug, type Found, type Ran } from "../lib/compose";
import { canDictate, dictate, type Dictation } from "../lib/dictation";
import { launchItem, startTask, toast, useApp } from "../store";
import type { Item } from "../proto";
import { AgentChip, composerDraft, lastRepo, RepoChip, rememberedRan, rememberStart, setComposerDraft, useOffered } from "./Composer";
import { useRepos } from "./Repos";
import { MAC, useWide } from "./Terminal";
import { QuotaMeter } from "./Quota";
import { Mic as MicIcon } from "./icons";

/**
 * Start agents (backlog #282): several tasks at once, a session each in its own
 * worktree. Opens with the home composer's lines; each row shows the repo and
 * agent it starts on, to change before anything runs. Backlog items join from
 * the side (a phone: "Pick from the backlog"), their repo fixed. A modal dialog
 * over the Inbox; Esc gives the lines back to the composer.
 */
export function StartAgents() {
  const wide = useWide();
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const status = useApp((s) => s.status);
  const offered = useOffered();
  const { list } = useRepos();
  const ctx: Context = {
    repos: (list?.repos ?? []).map((r) => r.path),
    last: lastRepo(),
    remembered: rememberedRan(),
    items,
    sessions,
    boot,
  };

  const [rows, setRows] = useState<Row[]>([]);
  const seeded = useRef(false);
  const [adding, setAdding] = useState("");
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [listening, setListening] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const add = useRef<HTMLTextAreaElement>(null);
  const mic = useRef<Dictation | null>(null);

  // The repos guess the rows: wait for them, then read the composer's draft once.
  useEffect(() => {
    if (seeded.current || !list) return;
    seeded.current = true;
    setRows(rowsFrom(composerDraft(), ctx));
  }, [list]); // the guess reads the rest as they are then

  useEffect(() => {
    const d = dialog.current;
    if (d && !d.open) d.showModal();
    // Over the Inbox's own keys from the first press: the add box, where typing goes.
    add.current?.focus();
    return () => mic.current?.stop();
  }, []);

  // A click on the sheet's blank space leaves the keyboard on <body>, outside the
  // dialog, where the Inbox's own keys would take it. Keys there are the sheet's.
  const latestClose = useRef(() => {});
  const latestStart = useRef(() => {});
  useEffect(() => {
    const stray = (e: KeyboardEvent) => {
      if (e.altKey || dialog.current?.contains(e.target as Node)) return;
      e.stopImmediatePropagation();
      if (e.key === "Escape") {
        e.preventDefault();
        latestClose.current();
      } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        latestStart.current();
      } else add.current?.focus();
    };
    window.addEventListener("keydown", stray, true);
    return () => window.removeEventListener("keydown", stray, true);
  }, []);

  useLayoutEffect(() => {
    const el = add.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [adding]);

  const picks = pickable(items, sessions, boot, offered, wide ? 6 : 8);
  const inRows = useMemo(() => new Set(rows.flatMap((r) => (r.item === null ? [] : [r.item]))), [rows]);
  // Drawn once (decision #77, rule 3): a picked item is a row, not a pick too.
  const left = picks.filter((f) => !inRows.has(f.item.id));
  const ready = left.filter((f) => f.pick === "start").length;
  const starts = rows.filter((r) => !r.why);
  const failed = rows.filter((r) => r.failed).length;
  const offline = status !== "open";

  const close = () => {
    mic.current?.stop();
    setComposerDraft([draft(rows, items), adding.trim()].filter(Boolean).join("\n"));
    location.hash = wide ? "#/new" : HASH.inbox;
  };
  latestClose.current = close;

  const addText = (text: string) => {
    const got = lines(text).map((l) => rowsFrom(l, ctx)[0]);
    if (got.length) setRows((r) => [...r, ...got]);
  };

  const change = (key: number, f: (r: Row) => Row) => setRows((rs) => rs.map((r) => (r.key === key ? { ...f(r), failed: null } : r)));
  // Another repo runs what it last ran, as a fresh line there would.
  const onRepo = (key: number, picked: string) => {
    const repo = picked || null;
    change(key, (r) => ({ ...r, repo, why: null, ran: lastRan(repo, ctx.remembered, items, sessions) }));
  };
  const drop = (key: number) => setRows((rs) => rs.filter((r) => r.key !== key));
  const toggle = (f: Found) => {
    if (f.pick === "blocked") return;
    if (inRows.has(f.item.id)) setRows((rs) => rs.filter((r) => r.item !== f.item.id));
    else setRows((rs) => [...rs, itemRow(f, ctx)]);
  };

  const start = async () => {
    if (busy || offline || starts.length === 0) return;
    mic.current?.stop();
    setBusy(true);
    const stay: Row[] = [];
    let n = 0;
    for (const r of rows) {
      if (r.why) {
        stay.push(r);
        continue;
      }
      try {
        if (r.item !== null) await launchItem(r.item, r.ran.agent, true, r.ran.model ?? "");
        else await startTask(command(r.ran, r.task), r.repo, slug(r.task));
        if (r.repo) rememberStart(r.repo, r.ran);
        n++;
      } catch (e) {
        stay.push({ ...r, failed: (e as Error).message });
      }
    }
    setBusy(false);
    const said = `Started ${n} session${n === 1 ? "" : "s"}`;
    if (stay.length === 0 && !adding.trim()) {
      setComposerDraft("");
      navigator.vibrate?.(10);
      toast(said);
      location.hash = HASH.inbox;
      return;
    }
    // What didn't start stays, with why, to change and try again.
    setRows(stay);
    if (n) toast(said);
  };

  latestStart.current = () => void start();

  const listen = () => {
    if (mic.current) {
      mic.current.stop();
      return;
    }
    let taken = 0;
    setListening(true);
    navigator.vibrate?.(10);
    mic.current = dictate(
      (settled, forming) => {
        const said = spoken(settled);
        // A pause settles a phrase: each becomes a row as it's heard.
        if (said.length > taken) {
          const more = said.slice(taken).map((t) => taskRow(t, ctx));
          taken = said.length;
          setRows((r) => [...r, ...more]);
        }
        setAdding(forming);
      },
      (error) => {
        mic.current = null;
        setListening(false);
        setAdding("");
        if (error === "not-allowed") toast("Allow the microphone to dictate");
        else if (error) toast(`Dictation stopped: ${error}`);
      },
      true,
    );
  };

  const rowEls = () => [...(dialog.current?.querySelectorAll<HTMLElement>(".start-row") ?? [])];
  const pickEls = () => [...(dialog.current?.querySelectorAll<HTMLInputElement>(".start-pick-row input:not(:disabled)") ?? [])];

  const onKey = (e: ReactKeyboardEvent<HTMLDialogElement>) => {
    // Alt keys still move between sessions; the rest are the dialog's, not the Inbox's behind it.
    if (e.altKey || e.nativeEvent.isComposing) return;
    e.stopPropagation();
    const mod = e.ctrlKey || e.metaKey;
    const t = e.target as HTMLElement;
    const row = t.closest<HTMLElement>(".start-row");
    if (e.key === "Escape") {
      e.preventDefault();
      if (row || t.closest(".start-pick-row")) add.current?.focus();
      else close();
      return;
    }
    if (mod && e.key === "Enter") {
      e.preventDefault();
      void start();
      return;
    }
    if (t === add.current) {
      // The box as it is now, not as last drawn: keys can come faster than renders.
      const typed = add.current.value;
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        addText(typed);
        setAdding("");
      } else if (mod && e.key === "Backspace" && !typed) {
        e.preventDefault();
        setRows((r) => r.slice(0, -1));
      } else if (e.key === "ArrowUp" && !typed) {
        e.preventDefault();
        rowEls().at(-1)?.focus();
      }
      return;
    }
    if (row && t === row) {
      const all = rowEls();
      const at = all.indexOf(row);
      const key = Number(row.dataset.key);
      if (e.key === "j" || e.key === "ArrowDown") {
        e.preventDefault();
        (all[at + 1] ?? add.current)?.focus();
      } else if (e.key === "k" || e.key === "ArrowUp") {
        e.preventDefault();
        all[Math.max(0, at - 1)]?.focus();
      } else if (e.key === "Enter") {
        e.preventDefault();
        void start();
      } else if (e.key === "d" || e.key === "Delete" || (mod && e.key === "Backspace")) {
        e.preventDefault();
        drop(key);
        (all[at + 1] ?? all[at - 1] ?? add.current)?.focus();
      }
      return;
    }
    // A click on blank space focuses the dialog itself: j/k reach the lines, the rest types.
    if (t === dialog.current) {
      const all = rowEls();
      if ((e.key === "k" || e.key === "ArrowUp") && all.length) {
        e.preventDefault();
        all[all.length - 1].focus();
      } else if ((e.key === "j" || e.key === "ArrowDown") && all.length) {
        e.preventDefault();
        all[0].focus();
      } else if (e.key.length === 1 && !mod) add.current?.focus();
      return;
    }
    const pick = t.closest<HTMLElement>(".start-pick-row");
    if (pick && (e.key === "j" || e.key === "k")) {
      e.preventDefault();
      const all = pickEls();
      const at = all.indexOf(t as HTMLInputElement);
      all[Math.max(0, Math.min(all.length - 1, at + (e.key === "j" ? 1 : -1)))]?.focus();
    } else if (pick && e.key === "Enter") {
      e.preventDefault();
      (t as HTMLInputElement).click();
    }
  };

  const mod = MAC ? "⌘" : "Ctrl";
  const go = starts.length === 1 ? "Start 1 agent" : `Start ${starts.length} agents`;

  const picker = (
    <section className="start-picks" aria-label="Ready in the backlog">
      <div className="start-picks-head">
        <h3>Ready in the backlog</h3>
        <span>next first</span>
      </div>
      {left.length === 0 && <p className="start-none">{picks.length ? "All of them are in the list" : "Nothing ready"}</p>}
      {left.map((f) => (
        <label key={f.item.id} className={`start-pick-row ${f.pick === "blocked" ? "blocked" : ""}`}>
          <input type="checkbox" checked={false} disabled={f.pick === "blocked"} onChange={() => toggle(f)} />
          <span className="start-ref">{itemRef(f.item)}</span>
          <span className="start-pick-title">{f.item.title || f.item.body.split("\n")[0]}</span>
          {f.why && <span className="start-why">{f.why}</span>}
        </label>
      ))}
      {wide && <p className="start-note">Ticking one adds it to the list, its repo fixed. One blocked by another stays grey until that one is in review.</p>}
    </section>
  );

  return (
    <dialog
      ref={dialog}
      className={`start-sheet ${wide ? "" : "phone"}`}
      aria-labelledby="start-title"
      onKeyDown={onKey}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
      onClick={(e) => {
        // A click on the backdrop, outside the sheet.
        if (e.target === dialog.current) close();
      }}
    >
      <div className="start-inner">
        {!wide && <div className="start-grab" aria-hidden="true" />}
        <header className="start-head">
          <h2 id="start-title">Start agents</h2>
          {wide && <span className="start-sub">one task per line, or pick from the backlog</span>}
          <span className="start-gap" />
          <button className="icon start-close" aria-label="Close" onClick={close}>
            ×
          </button>
        </header>
        <div className="start-body">
          <div className="start-main">
            {rows.length > 0 && (
              <ol className="start-rows" aria-label="Sessions to start">
                {rows.map((r, i) => (
                  <RowView key={r.key} row={r} n={i + 1} list={list} items={items} onChange={change} onRepo={onRepo} onDrop={drop} />
                ))}
              </ol>
            )}
            <div className="start-add">
              <span className="start-n" aria-hidden="true">
                {rows.length + 1}
              </span>
              <textarea
                ref={add}
                rows={1}
                value={adding}
                onChange={(e) => {
                  const v = e.target.value;
                  // A paste of several lines is several rows.
                  if (v.includes("\n")) {
                    addText(v);
                    setAdding("");
                  } else setAdding(v);
                }}
                placeholder={listening ? "Listening… pause between tasks" : rows.length ? "Another task…" : "Type a task, or paste a list…"}
                aria-label="Add a task"
                enterKeyHint="enter"
                readOnly={listening}
                spellCheck
              />
            </div>
            {wide && (
              <p className="start-note">
                The repo comes from words in the line, else there's none; the agent from what that repo last ran. A task in a repo gets its own branch and worktree; one with no repo starts in your home folder. A line can be a backlog number, like <code>web#7</code>.
              </p>
            )}
            {!wide && (
              <button className="start-pick" aria-expanded={picking} onClick={() => setPicking((p) => !p)}>
                <span className="start-plus" aria-hidden="true">
                  {picking ? "−" : "+"}
                </span>
                Pick from the backlog
                <span className="start-gap" />
                <span className="start-count">{ready} ready</span>
              </button>
            )}
            {!wide && picking && picker}
            {!wide && canDictate() && <p className="start-note">Dictate: a pause, or saying "next", starts another task.</p>}
          </div>
          {wide && <aside className="start-side">{picker}</aside>}
        </div>
        <footer className="start-foot">
          <span className="start-sum">
            {rows.length ? summary(starts) : "nothing yet"}
            {failed > 0 && <span className="start-failed"> · {failed} couldn't start</span>}
          </span>
          {wide && <QuotaMeter quiet />}
          <span className="start-gap" />
          {wide && (
            <span className="start-keys">
              <kbd>{mod}↵</kbd> start · <kbd>{mod}⌫</kbd> drop line
            </span>
          )}
          {canDictate() && (
            <button className={`start-mic ${listening ? "on" : ""}`} aria-label={listening ? "Stop dictating" : "Dictate tasks"} aria-pressed={listening} onClick={listen}>
              <MicIcon />
            </button>
          )}
          <button className="primary start-go" disabled={busy || offline || starts.length === 0} onClick={() => void start()}>
            {busy ? "Starting…" : go}
          </button>
        </footer>
      </div>
    </dialog>
  );
}

function RowView({
  row,
  n,
  list,
  items,
  onChange,
  onRepo,
  onDrop,
}: {
  row: Row;
  n: number;
  list: ReturnType<typeof useRepos>["list"];
  items: Item[];
  onChange: (key: number, f: (r: Row) => Row) => void;
  onRepo: (key: number, repo: string) => void;
  onDrop: (key: number) => void;
}) {
  const item = row.item === null ? undefined : items.find((i) => i.id === row.item);
  const why = row.failed ? `couldn't start: ${row.failed}` : row.why;
  return (
    <li className={`start-row ${row.why ? "stuck" : ""}`} tabIndex={0} data-key={row.key} aria-label={`${n}. ${row.task}${why ? `, ${why}` : ""}`}>
      <span className="start-n" aria-hidden="true">
        {item ? "" : n}
      </span>
      <span className="start-task">
        {item && <span className="start-ref">{itemRef(item)}</span>}
        {row.task}
      </span>
      <span className="start-chips">
        {item ? (
          <span className="start-repo-fixed" title="An item starts in its own repo">
            {row.repo ? base(row.repo) : "no repo"}
          </span>
        ) : (
          <RepoChip repo={row.repo} list={list} onPick={(repo) => onRepo(row.key, repo)} />
        )}
        <AgentChip ran={row.ran} onPick={(ran: Ran) => onChange(row.key, (r) => ({ ...r, ran }))} />
      </span>
      <button className="icon start-drop" aria-label={`Drop "${row.task}"`} tabIndex={-1} onClick={() => onDrop(row.key)}>
        ×
      </button>
      {why && <span className={`start-why ${row.failed ? "failed" : ""}`}>{why}</span>}
    </li>
  );
}
