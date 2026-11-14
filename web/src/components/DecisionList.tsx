import { useEffect, useRef, useState } from "react";
import type { Decision, Note } from "../proto";
import { SHELVES, type Shelf, byProject, ended, inFull, matches, pinnable, shelf } from "../lib/decisions";
import { live, notesByProject } from "../lib/notes";
import { scopeName } from "../lib/scope";
import { age } from "../lib/time";
import { allDecisions, allNotes, correctNote, deleteNote, review, useApp } from "../store";
import { DecisionEditor } from "./Decisions";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { Back } from "./icons";
import { Markdown } from "./Markdown";

/** Decisions and notes together; null when either can't be asked. */
export async function fetchMemory(): Promise<{ decisions: Decision[]; notes: Note[] } | null> {
  const [decisions, notes] = await Promise.all([allDecisions(), allNotes()]);
  return decisions && notes ? { decisions, notes } : null;
}

const home = () => {
  location.hash = "#/";
};

type Tab = Shelf | "notes";
const TABS: Tab[] = [...SHELVES, "notes"];

const ABOUT: Record<Tab, string> = {
  decisions:
    "Every agent gets constraints and the decisions you pin in full when it starts. The rest it gets as a title and reads when it needs to. Each pin takes up room in every session, so pin what holds everywhere.",
  skills: "Procedures you approved. Agents get each one's title, and read the steps when they do what it covers.",
  policies: "What Valkyrie does for you in this repository on its own, such as how far to ship and the checks to run first.",
  habits: "How you work in every project. They hold everywhere, so they have no scope and aren't pinned.",
  notes:
    "What agents noted about a repository, unreviewed. Never given to an agent unless it searches. Correct one to replace it, or delete it so it isn't written again.",
};

/**
 * Everything Valkyrie remembers for agents, to review, reword, retire or bring
 * back: decisions, skills, policies, your habits (ADR-0007, ADR-0016) and the
 * agents' notes (ADR-0017). Proposals wait in the inbox, not here.
 */
export function DecisionList() {
  const { data: memory, failed: why, reload: load } = useCached("memory", fetchMemory);
  const decisions = memory?.decisions ?? null;
  const notes = memory?.notes ?? null;
  const failed = why !== null;
  const [tab, setTab] = useState<Tab>("decisions");
  const [only, setOnly] = useState<string | null>(null);
  const [showEnded, setShowEnded] = useState(false);
  const [words, setWords] = useState("");
  const filter = useRef<HTMLInputElement>(null);


  // q or Esc goes back and / filters, when you aren't typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        home();
      } else if (e.key === "/") {
        e.preventDefault();
        filter.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onShelf = (decisions ?? []).filter((d) => tab !== "notes" && shelf(d) === tab);
  const counts = Object.fromEntries(
    TABS.map((t) => [
      t,
      t === "notes"
        ? (notes ?? []).filter((n) => live(n) !== showEnded).length
        : (decisions ?? []).filter((d) => shelf(d) === t && (showEnded ? ended(d) : d.status === "active")).length,
    ]),
  ) as Record<Tab, number>;

  const decisionGroups = byProject(
    onShelf.filter((d) => matches(words, d.title, d.body, d.scope?.join(" "))),
    showEnded,
  );
  const noteGroups = notesByProject(
    (notes ?? []).filter((n) => matches(words, n.text)),
    showEnded,
  );
  const groups: [string, unknown[]][] = tab === "notes" ? noteGroups : decisionGroups;
  // The project picker lists every project on this tab, whatever the filter says.
  const projects = [
    ...new Set(
      tab === "notes" ? (notes ?? []).map((n) => n.project) : onShelf.map((d) => d.project),
    ),
  ].sort((a, b) => scopeName(a).localeCompare(scopeName(b)));
  const shown = groups.filter(([p]) => only === null || p === only);
  const loaded = tab === "notes" ? notes !== null : decisions !== null;

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Memory</div>
        </div>
      </header>

      <main className="inbox">
        <div className="kind-pick memory-tabs" role="tablist" aria-label="What">
          {TABS.map((t) => (
            <button
              key={t}
              role="tab"
              aria-selected={tab === t}
              className={`kind-chip ${tab === t ? "on" : ""}`}
              onClick={() => {
                setTab(t);
                setOnly(null);
              }}
            >
              {t} <span className="count">{counts[t]}</span>
            </button>
          ))}
        </div>
        <p className="decisions-about">{ABOUT[tab]}</p>
        <div className="memory-filters">
          <input
            ref={filter}
            className="memory-filter"
            type="search"
            placeholder="Filter by words"
            aria-label="Filter by words"
            value={words}
            onChange={(e) => setWords(e.target.value)}
            spellCheck={false}
          />
          <div className="kind-pick" role="radiogroup" aria-label="Which">
            {[false, true].map((e) => (
              <button
                key={String(e)}
                role="radio"
                aria-checked={showEnded === e}
                className={`kind-chip ${showEnded === e ? "on" : ""}`}
                onClick={() => setShowEnded(e)}
              >
                {e ? (tab === "notes" ? "replaced" : "ended") : tab === "notes" ? "live" : "in use"}
              </button>
            ))}
          </div>
        </div>
        {projects.length > 1 && (
          <div className="kind-pick" role="radiogroup" aria-label="Project">
            {[null, ...projects].map((p) => (
              <button
                key={p ?? "all"}
                role="radio"
                aria-checked={only === p}
                className={`kind-chip ${only === p ? "on" : ""}`}
                onClick={() => setOnly(p)}
              >
                {p === null ? "all" : scopeName(p)}
              </button>
            ))}
          </div>
        )}
        {!loaded && (failed ? <div className="empty">Couldn't load them.</div> : <Loading rows={5} />)}
        {loaded && shown.length === 0 && (
          <div className="empty">{words.trim() ? "Nothing matches those words." : `No ${showEnded ? "ended " : ""}${tab} yet.`}</div>
        )}
        {shown.map(([project, items]) => (
          <section key={project}>
            <h2 className="section-title">
              {scopeName(project)} <span className="count">{items.length}</span>
            </h2>
            <div className="cards">
              {tab === "notes"
                ? (items as Note[]).map((n) => <NoteRow key={`${n.project}#${n.id}`} n={n} onChange={load} />)
                : (items as Decision[]).map((d) => (
                    <DecisionRow key={`${d.project}#${d.id}`} d={d} onChange={load} />
                  ))}
            </div>
          </section>
        ))}
      </main>
    </div>
  );
}

/** Where it came from, as `valk decisions show` prints it. */
function Provenance({ d }: { d: Decision }) {
  const now = useApp((s) => s.now);
  const p = d.provenance;
  const by = { human: "you", valkyrie: "Valkyrie" }[p.by] ?? p.by;
  return (
    <div className="decision-by">
      by {by}
      {p.session && <> · {p.session}</>}
      {p.commit && <> · {p.commit.slice(0, 7)}</>}
      {" · "}
      {d.fresh?.confirmed ? <>confirmed {age(d.fresh.confirmed * 1000, now)} ago</> : <>changed {age(d.updated * 1000, now)} ago</>}
      {d.supersedes !== undefined && <> · replaced #{d.supersedes}</>}
      {d.superseded_by !== undefined && <> · replaced by #{d.superseded_by}</>}
    </div>
  );
}

function DecisionRow({ d, onChange }: { d: Decision; onChange: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  // Retiring asks twice: agents stop getting it from their next session.
  const [sure, setSure] = useState(false);
  // Read again either way: a refusal ("changed since you read it") is fixed by
  // what's there now.
  const act = async (action: Parameters<typeof review>[1]) => {
    setBusy(true);
    try {
      await review(d, action);
      await onChange();
    } finally {
      setBusy(false);
      setSure(false);
    }
  };
  const active = d.status === "active";
  const how = !active ? d.status : d.kind === "constraint" ? "always in full" : d.pinned ? "pinned" : null;
  return (
    <div className="card-slot">
      <article className={`card decision-card ${active && inFull(d) ? "in-full" : ""} ${active ? "" : "ended"}`}>
        <div className="card-head">
          <span className={`kind-chip kind-${d.kind}`}>{d.kind}</span>
          <span className="card-name">#{d.id}</span>
          {how && <span className="card-prog">{how}</span>}
        </div>
        {editing ? (
          <DecisionEditor
            d={d}
            busy={busy}
            onDone={() => {
              setEditing(false);
              void onChange();
            }}
          />
        ) : (
          <>
            <button className="decision-title as-text" aria-expanded={open} onClick={() => setOpen(!open)}>
              {d.title}
            </button>
            {open && d.body && <Markdown text={d.body} className={`decision-body ${d.kind}`} />}
            {d.scope?.length ? <div className="decision-scope">applies to {d.scope.join(", ")}</div> : null}
            {open && <Provenance d={d} />}
            {active && (
              <div className="card-actions">
                {open && (
                  <button
                    className={`ghost no ${sure ? "item-sure" : ""}`}
                    disabled={busy}
                    onClick={() => (sure ? void act({ a: "retire" }) : setSure(true))}
                  >
                    {sure ? "Retire for good?" : "Retire"}
                  </button>
                )}
                {open && (
                  <button className="ghost" disabled={busy} onClick={() => setEditing(true)}>
                    Edit
                  </button>
                )}
                {pinnable(d) && (
                  <button
                    className={d.pinned ? "ghost" : "primary"}
                    disabled={busy}
                    onClick={() => void act({ a: d.pinned ? "unpin" : "pin" })}
                  >
                    {d.pinned ? "Unpin" : "Pin"}
                  </button>
                )}
              </div>
            )}
            {d.status === "retired" && (
              <div className="card-actions">
                <button className="primary" disabled={busy} onClick={() => void act({ a: "restore" })}>
                  Restore
                </button>
              </div>
            )}
          </>
        )}
      </article>
    </div>
  );
}

function NoteRow({ n, onChange }: { n: Note; onChange: () => Promise<void> }) {
  const now = useApp((s) => s.now);
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(n.text);
  const [busy, setBusy] = useState(false);
  const [sure, setSure] = useState(false);
  const run = async (f: () => Promise<boolean>) => {
    setBusy(true);
    try {
      if (await f()) {
        setEditing(false);
        await onChange();
      }
    } finally {
      setBusy(false);
      setSure(false);
    }
  };
  const by = { user: "you", valkyrie: "Valkyrie's retro", agent: "an agent" }[n.by] ?? n.by;
  const state = n.status !== "live" ? n.status : n.stale ? "stale" : null;
  return (
    <div className="card-slot">
      <article className={`card decision-card note-card ${n.stale ? "stale" : ""} ${live(n) ? "" : "ended"}`}>
        <div className="card-head">
          <span className="kind-chip kind-note">note</span>
          <span className="card-name">#{n.id}</span>
          {state && <span className="card-prog">{state}</span>}
          <span className="card-age">{age(n.created * 1000, now)}</span>
        </div>
        {editing ? (
          <div className="decision-edit">
            <label className="field">
              <span className="field-label">Corrected note (replaces #{n.id})</span>
              <textarea value={text} onChange={(e) => setText(e.target.value)} rows={4} maxLength={1000} />
            </label>
            <div className="card-actions">
              <button className="ghost" disabled={busy} onClick={() => setEditing(false)}>
                Cancel
              </button>
              <button
                className="primary"
                disabled={busy || !text.trim() || text.trim() === n.text.trim()}
                onClick={() => void run(() => correctNote(n, text.trim()))}
              >
                Replace
              </button>
            </div>
          </div>
        ) : (
          <>
            <button className="note-text as-text" aria-expanded={open} onClick={() => setOpen(!open)}>
              {n.text}
            </button>
            {n.stale && <div className="decision-by">may be out of date: {n.stale}</div>}
            {open && (
              <div className="decision-by">
                by {by} · {n.session === "none" ? "outside a session" : n.session}
                {n.commit && <> · {n.commit.slice(0, 7)}</>}
                {n.replaces != null && <> · replaced #{n.replaces}</>}
                {n.replaced_by != null && <> · replaced by #{n.replaced_by}</>}
                {n.anchors?.length ? <> · about {n.anchors.join(", ")}</> : null}
              </div>
            )}
            {open && live(n) && (
              <div className="card-actions">
                <button
                  className={`ghost no ${sure ? "item-sure" : ""}`}
                  disabled={busy}
                  onClick={() => (sure ? void run(() => deleteNote(n)) : setSure(true))}
                >
                  {sure ? "Delete for good" : "Delete"}
                </button>
                <button
                  className="ghost"
                  disabled={busy}
                  onClick={() => {
                    setText(n.text);
                    setEditing(true);
                  }}
                >
                  Correct
                </button>
              </div>
            )}
          </>
        )}
      </article>
    </div>
  );
}
