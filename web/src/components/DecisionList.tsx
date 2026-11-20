import { useEffect, useRef, useState } from "react";
import { MemoryTabs } from "./Nav";
import type { Decision, Note } from "../proto";
import { byProject, inFull, layerWords, matches, pinnable, served, shelf as shelfOf } from "../lib/decisions";
import { live, notesByProject } from "../lib/notes";
import { HASH, SHELF_HASH, type MemoryShelf } from "../lib/routes";
import { USER_SCOPE, scopeName } from "../lib/scope";
import { age } from "../lib/time";
import { allDecisions, allNotes, correctNote, deleteNote, review, servedTo, useApp } from "../store";
import { DecisionEditor, ToReview } from "./Decisions";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back, Chevron } from "./icons";
import { Markdown } from "./Markdown";
import { usePageKeys } from "../lib/pageKeys";
import { useDesktop, useWide } from "./Terminal";

/** Decisions and notes together, either missing when it can't be asked; null
 * when neither can. */
export async function fetchMemory(): Promise<{ decisions: Decision[] | null; notes: Note[] | null } | null> {
  const [decisions, notes] = await Promise.all([allDecisions(), allNotes()]);
  return decisions || notes ? { decisions, notes } : null;
}

const home = () => {
  location.hash = "#/";
};

/** What each shelf is called on a phone's overview and in an empty state. */
const NAME: Record<MemoryShelf, string> = {
  decisions: "Decisions",
  skills: "Skills",
  policies: "Policies",
  habits: "Your habits",
  notes: "Notes",
};

/** What an empty shelf says to do next. Agents' proposals show under To review. */
const EMPTY_HINT: Record<MemoryShelf, string> = {
  decisions: "Agents propose decisions as they work; the ones you accept land here.",
  skills: "Agents propose procedures worth repeating; the ones you accept land here.",
  policies: "Agents propose policies; the ones you accept land here.",
  habits: "Valkyrie learns these from how you work, and they land here once you accept them.",
  notes: "Agents leave notes about a repository as they work.",
};

const ABOUT: Record<MemoryShelf, string> = {
  decisions:
    "Every agent gets constraints and the decisions you pin in full when it starts. The rest it gets as a title and reads when it needs to. Each pin takes up room in every session, so pin what holds everywhere.",
  skills: "Procedures you approved. Agents get each one's title, and read the steps when they do what it covers.",
  policies: "What Valkyrie does for you in this repository on its own, such as how far to ship and the checks to run first.",
  habits: "How you work in every project. They hold everywhere, so they have no scope and aren't pinned.",
  notes:
    "What agents noted about a repository, unreviewed. Never given to an agent unless it searches. Correct one to replace it, or delete it so it isn't written again.",
};

/** Read the memory again once the daemon says a proposal was answered (here or
 * anywhere), not when its card leaves: that's before the undo runs out. */
function useMemory() {
  const cached = useCached("memory", fetchMemory);
  const pushes = useApp((s) => s.proposalPushes);
  const seen = useRef(pushes);
  const { reload } = cached;
  useEffect(() => {
    if (seen.current === pushes) return;
    seen.current = pushes;
    void reload();
  }, [pushes, reload]);
  return cached;
}

/**
 * Everything Valkyrie remembers for agents, to review, reword, retire or bring
 * back: decisions, skills, policies, your habits (ADR-0007, ADR-0016) and the
 * agents' notes (ADR-0017), each a tab (#253). Proposals wait on top, under To
 * review. A phone lands on an overview of the shelves instead of on Decisions.
 */
export function DecisionList({ shelf }: { shelf: MemoryShelf | null }) {
  const desktop = useDesktop();
  if (shelf === null && !desktop) return <MemoryHome />;
  return <ShelfPage key={shelf ?? "decisions"} tab={shelf ?? "decisions"} />;
}

/** A phone's Memory: what waits on you, then the shelves in two lists, what
 * every agent is told and what's written down. */
function MemoryHome() {
  const { data: memory } = useMemory();
  const unread = useApp((s) => s.unreadDocs);
  const decisions = memory?.decisions ?? null;
  const notes = memory?.notes ?? null;
  // Only a phone lands here, so the list keys have nothing to pick.
  const none = () => {};
  usePageKeys("decisions", { q: home, Escape: home, "/": none, j: none, k: none, Enter: none });
  const inUse = (t: MemoryShelf) =>
    t === "notes" ? notes?.filter(live).length : decisions?.filter((d) => d.status === "active" && shelfOf(d) === t).length;
  const told: MemoryShelf[] = ["decisions", "skills", "policies", "habits"];
  const link = (label: string, hash: string, n?: number, sub?: string) => (
    <a key={hash} className="row mem-link" href={hash}>
      <span className="row-main">
        <span className="row-name">{label}</span>
        {sub && <span className="row-sub">{sub}</span>}
      </span>
      {n !== undefined && <span className="count">{n}</span>}
      <Chevron />
    </a>
  );
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Memory</h1>
        </div>
      </header>
      <main className="inbox">
        <ToReview />
        <h2 className="section-title">Told to every agent</h2>
        <nav className="list" aria-label="Told to every agent">
          {told.map((t) => link(NAME[t], SHELF_HASH[t], inUse(t)))}
        </nav>
        <h2 className="section-title">Written down</h2>
        <nav className="list" aria-label="Written down">
          {link("Docs", HASH.docs, undefined, unread ? `${unread} unread` : undefined)}
          {link("Wiki", HASH.wiki)}
          {link(NAME.notes, SHELF_HASH.notes, inUse("notes"), "never told; agents find them by searching")}
        </nav>
      </main>
    </div>
  );
}

/** A row on a shelf: a decision or a note, keyed as the list picks it. */
type Row = { key: string; d: Decision; n?: undefined } | { key: string; n: Note; d?: undefined };

function ShelfPage({ tab }: { tab: MemoryShelf }) {
  const { data: memory, failed: why, reload: load } = useMemory();
  const decisions = memory?.decisions ?? null;
  const notes = memory?.notes ?? null;
  const desktop = useDesktop();
  const wide = useWide();
  const [only, setOnly] = useState<string | null>(null);
  const [showEnded, setShowEnded] = useState(false);
  const [words, setWords] = useState("");
  const filter = useRef<HTMLInputElement>(null);

  const onShelf = (decisions ?? []).filter((d) => tab !== "notes" && shelfOf(d) === tab);
  const decisionGroups = byProject(
    onShelf.filter((d) => matches(words, d.title, d.body, d.scope?.join(" "))),
    showEnded,
  );
  const noteGroups = notesByProject(
    (notes ?? []).filter((n) => matches(words, n.text)),
    showEnded,
  );
  const groups: [string, Row[]][] =
    tab === "notes"
      ? noteGroups.map(([p, ns]) => [p, ns.map((n) => ({ key: `n${n.project}#${n.id}`, n }))])
      : decisionGroups.map(([p, ds]) => [p, ds.map((d) => ({ key: `${d.project}#${d.id}`, d }))]);
  // The project picker lists every project on this tab, whatever the filter says.
  const projects = [
    ...new Set(
      tab === "notes" ? (notes ?? []).map((n) => n.project) : onShelf.map((d) => d.project),
    ),
  ].sort((a, b) => scopeName(a).localeCompare(scopeName(b)));
  const shown = groups.filter(([p]) => only === null || p === only);
  const loaded = tab === "notes" ? notes !== null : decisions !== null;

  // j/k pick a row and Enter opens it: in the column beside the list when
  // there's room, under its row when there isn't. Esc closes it, then goes back.
  const rows = shown.flatMap(([, rs]) => rs);
  const [sel, setSel] = useState(-1);
  const [opened, setOpened] = useState<string | null>(null);
  const open = rows.find((r) => r.key === opened) ?? null;
  const beside = wide && open !== null;
  const pickRow = (i: number) => {
    const next = Math.max(0, Math.min(rows.length - 1, i));
    setSel(next);
    // With one open beside the list, it follows what's picked, as a reader does.
    if (beside && rows[next]) setOpened(rows[next].key);
    document.querySelectorAll<HTMLElement>(".mem-slot")[next]?.scrollIntoView({ block: "nearest" });
  };
  const toggle = (key: string) => {
    setOpened((was) => (was === key ? null : key));
    setSel(rows.findIndex((r) => r.key === key));
  };
  const close = () => setOpened(null);
  usePageKeys("decisions", {
    q: home,
    Escape: () => (opened !== null ? close() : home()),
    "/": () => filter.current?.focus(),
    j: () => pickRow(sel + 1),
    k: () => pickRow(sel - 1),
    Enter: () => rows[sel] && toggle(rows[sel].key),
  });

  const page = (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={desktop ? home : () => (location.hash = HASH.memory)}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Memory</h1>
        </div>
      </header>
      <MemoryTabs />

      <main className="inbox">
        {tab !== "notes" && <ToReview />}
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
          <div className="seg" role="radiogroup" aria-label="Which">
            {[false, true].map((e) => (
              <button
                key={String(e)}
                role="radio"
                aria-checked={showEnded === e}
                className={`seg-item ${showEnded === e ? "on" : ""}`}
                onClick={() => setShowEnded(e)}
              >
                {e ? (tab === "notes" ? "replaced" : "ended") : tab === "notes" ? "live" : "in use"}
              </button>
            ))}
          </div>
        </div>
        {projects.length > 1 && (
          <div className="seg" role="radiogroup" aria-label="Project">
            {[null, ...projects].map((p) => (
              <button
                key={p ?? "all"}
                role="radio"
                aria-checked={only === p}
                className={`seg-item ${only === p ? "on" : ""}`}
                onClick={() => setOnly(p)}
              >
                {p === null ? "all" : scopeName(p)}
              </button>
            ))}
          </div>
        )}
        {!loaded && (why !== null ? <ErrorState what={tab === "notes" ? "the notes" : "what Valkyrie remembers"} why={why} onRetry={load} /> : <Loading rows={5} />)}
        {loaded && shown.length === 0 && (
          <EmptyState
            title={words.trim() ? "Nothing matches those words" : `No ${showEnded ? "ended " : ""}${NAME[tab].toLowerCase()} yet`}
            hint={words.trim() || showEnded || only !== null ? undefined : EMPTY_HINT[tab]}
            action={
              words.trim()
                ? { label: "Clear the filter", run: () => setWords("") }
                : showEnded
                  ? { label: "Show what's in use", run: () => setShowEnded(false) }
                  : only !== null
                    ? { label: "Show every project", run: () => setOnly(null) }
                    : undefined
            }
          />
        )}
        {shown.map(([project, items]) => (
          <section key={project} className="mem-group">
            <h2 className="section-title">
              {scopeName(project)} <span className="count">{items.length}</span>
            </h2>
            <div className="list mem-list">
              {items.map((r) => {
                const picked = rows[sel]?.key === r.key;
                const isOpen = opened === r.key;
                return (
                  <div key={r.key} className={`mem-slot ${picked ? "picked" : ""}`}>
                    {r.d ? (
                      <DecisionRow d={r.d} tab={tab} picked={picked} open={isOpen} current={isOpen && wide} onToggle={() => toggle(r.key)} />
                    ) : (
                      <NoteRow n={r.n} picked={picked} open={isOpen} current={isOpen && wide} onToggle={() => toggle(r.key)} />
                    )}
                    {isOpen && !wide && <Detail row={r} onChange={load} inline />}
                  </div>
                );
              })}
            </div>
          </section>
        ))}
      </main>
    </div>
  );
  if (!beside || !open) return page;
  return (
    <div className="split split-side">
      {page}
      <div className="split-pane">
        <div className="mem-pane">
          <button className="ghost mem-pane-close" title="Close (Esc)" onClick={close}>
            Close
          </button>
          <Detail key={open.key} row={open} onChange={load} />
        </div>
      </div>
    </div>
  );
}

/** `inline` under its row, which already says its title and kind: the rest only. */
function Detail({ row, onChange, inline = false }: { row: Row; onChange: () => Promise<void>; inline?: boolean }) {
  return row.d ? (
    <DecisionDetail d={row.d} onChange={onChange} inline={inline} />
  ) : (
    <NoteDetail n={row.n} onChange={onChange} inline={inline} />
  );
}

/** A decision's row: its number, title, and how it reaches agents. */
function DecisionRow({
  d,
  tab,
  picked,
  open,
  current,
  onToggle,
}: {
  d: Decision;
  tab: MemoryShelf;
  picked: boolean;
  open: boolean;
  /** Its detail is in the column beside the list. */
  current: boolean;
  onToggle: () => void;
}) {
  const now = useApp((s) => s.now);
  const how = served(d);
  return (
    <button
      className={`row mem-row ${picked ? "picked" : ""} ${current ? "current" : ""}`}
      aria-expanded={open}
      aria-current={current || undefined}
      onClick={onToggle}
    >
      <span className="item-ref">#{d.id}</span>
      <span className="row-main">
        <span className="row-name">{d.title}</span>
        <span className="row-sub">
          <span className={`state-tag ${how.full ? "served-full" : ""} ${d.status === "active" ? "" : "served-ended"}`}>{how.word}</span>
          {/* Decisions hold every kind but skills and policies; the other shelves are one kind. */}
          {tab === "decisions" && <span className="tag">{d.kind}</span>}
          {d.fresh?.review && <span className="tag">still true?</span>}
        </span>
      </span>
      <span className="row-age">{age(d.updated * 1000, now)}</span>
    </button>
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

/** A decision in full: its body, scope, how it's served and where it came from,
 * with Pin, Edit and Retire (or Restore). */
function DecisionDetail({ d, onChange, inline }: { d: Decision; onChange: () => Promise<void>; inline: boolean }) {
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
  return (
    <article className={`card decision-card mem-detail ${active && inFull(d) ? "in-full" : ""} ${active ? "" : "ended"}`}>
      {!inline && (
        <div className="card-head">
          <span className="tag">{d.kind}</span>
          <span className="card-name">#{d.id}</span>
          <span className="card-prog">{scopeName(d.project)}</span>
        </div>
      )}
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
          {!inline && <h2 className="decision-title">{d.title}</h2>}
          {d.body && <Markdown text={d.body} className={`decision-body ${d.kind}`} />}
          <dl className="mem-facts">
            {d.scope?.length ? (
              <>
                <dt>Applies to</dt>
                <dd className="mem-mono">{d.scope.join(", ")}</dd>
              </>
            ) : null}
            <dt>Served</dt>
            <dd>{served(d).line}</dd>
            {d.fresh?.review && (
              <>
                <dt>Still true?</dt>
                <dd>{d.fresh.review}</dd>
              </>
            )}
          </dl>
          {d.project !== USER_SCOPE && <Reached d={d} />}
          <Provenance d={d} />
          {active && (
            <div className="card-actions">
              <button
                className={`ghost no ${sure ? "item-sure" : ""}`}
                disabled={busy}
                onClick={() => (sure ? void act({ a: "retire" }) : setSure(true))}
              >
                {sure ? "Retire for good?" : "Retire"}
              </button>
              <button className="ghost" disabled={busy} onClick={() => setEditing(true)}>
                Edit
              </button>
              {pinnable(d) && (
                <button className="ghost" disabled={busy} onClick={() => void act({ a: d.pinned ? "unpin" : "pin" })}>
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
  );
}

/** The sessions it reached lately, from the serving log (backlog #256): each
 * once, how, and when last; a running one opens. Habits hold in every project,
 * so no one project's log has them. */
function Reached({ d }: { d: Decision }) {
  const now = useApp((s) => s.now);
  const { data: list, failed, reload } = useCached(`served:${d.project}#${d.id}`, () => servedTo(d));
  return (
    <section className="mem-reached" aria-label="Sessions it reached">
      <h3 className="mem-reached-head">
        Reached {list && list.length > 0 && <span className="count">{list.length}</span>}
      </h3>
      {!list && (failed ? <ErrorState what="where it was served" why={failed} onRetry={reload} /> : <Loading rows={2} />)}
      {list && list.length === 0 && (
        <p className="mem-reached-none">
          {d.status === "active" ? "No session got it in the last 90 days." : "No session got it in the last 90 days it was in use."}
        </p>
      )}
      {list && list.length > 0 && (
        <ul className="mem-reached-list">
          {list.map((r, i) => (
            <li key={`${r.what}:${r.first}:${i}`}>
              {r.session !== undefined ? (
                <a className="mem-reached-what" href={`#/s/${r.session}`}>
                  {r.what}
                </a>
              ) : (
                <span className="mem-reached-what">{r.what}</span>
              )}
              <span className="mem-reached-how">
                {[r.agent, layerWords(r.layers), r.session !== undefined ? "running" : null].filter(Boolean).join(" · ")}
              </span>
              <span className="row-age">{age(r.last * 1000, now)}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const noteBy = (n: Note) => ({ user: "you", valkyrie: "Valkyrie's retro", agent: "an agent" })[n.by] ?? n.by;
const noteState = (n: Note) => (n.status !== "live" ? n.status : n.stale ? "stale" : "live");

/** A note's row: its text, whether it's live or stale, and who wrote it. */
function NoteRow({
  n,
  picked,
  open,
  current,
  onToggle,
}: {
  n: Note;
  picked: boolean;
  open: boolean;
  current: boolean;
  onToggle: () => void;
}) {
  const now = useApp((s) => s.now);
  const state = noteState(n);
  return (
    <button
      className={`row mem-row ${picked ? "picked" : ""} ${current ? "current" : ""}`}
      aria-expanded={open}
      aria-current={current || undefined}
      onClick={onToggle}
    >
      <span className="item-ref">#{n.id}</span>
      <span className="row-main">
        <span className="row-name">{n.text}</span>
        <span className="row-sub">
          <span className={`state-tag ${state === "stale" ? "state-text-stale" : state === "live" ? "" : "served-ended"}`}>{state}</span>
          <span className="tag">by {noteBy(n)}</span>
        </span>
      </span>
      <span className="row-age">{age(n.created * 1000, now)}</span>
    </button>
  );
}

/** A note in full, to correct (it's replaced) or delete. */
function NoteDetail({ n, onChange, inline }: { n: Note; onChange: () => Promise<void>; inline: boolean }) {
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
  return (
    <article className={`card decision-card note-card mem-detail ${n.stale ? "stale" : ""} ${live(n) ? "" : "ended"}`}>
      {!inline && (
        <div className="card-head">
          <span className="tag">note</span>
          <span className="card-name">#{n.id}</span>
          <span className="card-prog">{scopeName(n.project)}</span>
        </div>
      )}
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
          {!inline && <p className="note-text">{n.text}</p>}
          <dl className="mem-facts">
            {n.stale && (
              <>
                <dt>May be out of date</dt>
                <dd>{n.stale}</dd>
              </>
            )}
            {n.anchors?.length ? (
              <>
                <dt>About</dt>
                <dd className="mem-mono">{n.anchors.join(", ")}</dd>
              </>
            ) : null}
            <dt>Served</dt>
            <dd>Never told to agents; one finds it by searching.</dd>
          </dl>
          <div className="decision-by">
            by {noteBy(n)} · {n.session === "none" ? "outside a session" : n.session}
            {n.commit && <> · {n.commit.slice(0, 7)}</>}
            {n.replaces != null && <> · replaced #{n.replaces}</>}
            {n.replaced_by != null && <> · replaced by #{n.replaced_by}</>}
          </div>
          {live(n) && (
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
  );
}
