import { useEffect, useMemo, useRef, useState } from "react";
import { HistoryTabs } from "./Nav";
import type { PastSession, SearchHit, SearchResults } from "../proto";
import { base } from "../lib/backlog";
import { clockOf, dayLabel, spanOf } from "../lib/digest";
import {
  byDay,
  historyStart,
  HISTORY_WINDOWS,
  shownCounts,
  shownLabel,
  tileCounts,
  tileOf,
  TILES,
  timeline,
  type HistoryWindow,
  type Tile,
  type TimelineEntry,
} from "../lib/history";
import { fieldLabel, marks, terms } from "../lib/search";
import { age } from "../lib/time";
import { awayDigest, pastSessions, resume, searchHistory, spawn, supervisorLog, toast, useApp } from "../store";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back } from "./icons";
import { usePageKeys } from "../lib/pageKeys";

const home = () => {
  location.hash = "#/";
};

/** How long to wait for typing to stop before asking the daemon. */
const SETTLE_MS = 250;

/** Copies `text`, falling back to a selected textarea where the clipboard API
 * isn't offered (a page served over plain http). */
async function copy(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to the old way.
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  const ok = document.execCommand("copy");
  area.remove();
  return ok;
}

/** Opens an ended session again on its own conversation, in a new tab, adding
 * its worktree back if it was taken away (backlog #150). */
async function resumePast(p: PastSession): Promise<void> {
  if (p.cannot) {
    toast(`Can't resume it: ${p.cannot}`);
    return;
  }
  try {
    const id = await resume(p.key);
    location.hash = `#/s/${id}`;
  } catch (e) {
    toast(`Resume failed: ${(e as Error).message}`);
  }
}

/**
 * History as one timeline (#254): what every agent did (the away digest, #105),
 * with Resume on the ones that ended (#150), and what the supervisor did (#180),
 * newest first by day. A window picks how far back; tiles count what finished,
 * what's stuck and what shipped, and filter to it. Typing searches every
 * session's words instead (#106).
 */
export function HistoryView({ find = false }: { find?: boolean }) {
  const queue = useApp((s) => s.queue);
  const [win, setWin] = useState<HistoryWindow>("away");
  const [tile, setTile] = useState<Tile | null>(null);
  const [query, setQuery] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (find) input.current?.focus();
  }, [find]);

  const digest = useCached(`digest:${win}`, () => awayDigest(historyStart(win, Date.now()), "day"));
  const past = useCached("past", pastSessions);
  const log = useCached("supervisor-log", supervisorLog);
  // An answer in the Inbox adds to what the supervisor did: read it again then.
  const pending = useApp((s) => s.launches.length + s.orders.length + s.queue.filter((q) => q.flag?.step).length);
  const first = useRef(true);
  const reloadLog = log.reload;
  useEffect(() => {
    if (first.current) first.current = false;
    else void reloadLog();
  }, [pending, reloadLog]);

  // Search the window the timeline shows; before it loads, as far as the daemon looks.
  const since = digest.data?.since_ms ?? null;
  const list = useMemo(() => {
    if (!digest.data) return null;
    const asking = (id: number) => queue.some((q) => q.session === id && q.status.state === "needs_input");
    return timeline({
      entries: digest.data.entries,
      past: past.data ?? [],
      log: log.data?.entries ?? [],
      since: digest.data.since_ms,
      asking,
    });
  }, [digest.data, past.data, log.data, queue]);
  const tiles = list ? tileCounts(list) : null;
  const shown = list?.filter((t) => tile === null || tileOf(t) === tile) ?? null;

  const searching = terms(query).length > 0;
  const search = useSearch(query, since);

  // j/k pick a row, Enter does its one thing (open, resume, the PR), / searches,
  // Esc clears the search and then goes back.
  const [sel, setSel] = useState(-1);
  const slots = () => document.querySelectorAll<HTMLElement>(".hist-slot");
  const pick = (i: number) => {
    const all = slots();
    const next = Math.max(0, Math.min(all.length - 1, i));
    setSel(next);
    all[next]?.scrollIntoView({ block: "nearest" });
  };
  useEffect(() => setSel(-1), [win, tile, searching]);
  usePageKeys(
    "past",
    {
      q: home,
      Escape: () => (query ? setQuery("") : home()),
      "/": () => input.current?.focus(),
      j: () => pick(sel + 1),
      k: () => pick(sel - 1),
      Enter: () => {
        // A running session's row is its own button; an ended one holds Resume, a step its PR link.
        const row = slots()[sel];
        (row?.matches("button") ? row : row?.querySelector<HTMLElement>("button:not(:disabled), a"))?.click();
      },
    },
    { blurOnEscape: true },
  );

  const onCopy = async () => {
    if (!digest.data) return;
    toast((await copy(digest.data.markdown)) ? "Copied as markdown." : "Couldn't copy it here.");
  };

  let slot = 0;
  const next = () => slot++;
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">History</h1>
        </div>
        <button
          className="ghost"
          title="What agents did in this window, as markdown for a standup"
          disabled={!digest.data || digest.data.entries.length === 0}
          onClick={() => void onCopy()}
        >
          Copy
        </button>
      </header>
      <HistoryTabs />

      <main className="inbox history">
        <div className="field search-box">
          <input
            ref={input}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder='Search what you or an agent said, a file, a branch, "a phrase"'
            aria-label="Search session history"
            enterKeyHint="search"
            spellCheck={false}
            autoCapitalize="off"
          />
        </div>
        <div className="seg" role="radiogroup" aria-label="How far back">
          {HISTORY_WINDOWS.map(([w, label]) => (
            <button
              key={w}
              role="radio"
              aria-checked={win === w}
              className={`seg-item ${win === w ? "on" : ""}`}
              onClick={() => setWin(w)}
            >
              {label}
            </button>
          ))}
        </div>

        {searching ? (
          <SearchResultsList search={search} words={terms(query)} next={next} sel={sel} onWider={win === "month" ? null : () => setWin("month")} />
        ) : (
          <>
            {tiles && (
              <div className="hist-tiles" role="group" aria-label="Outcomes">
                {TILES.map(([t, label]) => (
                  <button
                    key={t}
                    className={`hist-tile tile-${t} ${tile === t ? "on" : ""}`}
                    aria-pressed={tile === t}
                    disabled={tiles[t] === 0 && tile !== t}
                    onClick={() => setTile(tile === t ? null : t)}
                  >
                    <span className="hist-tile-n">{tiles[t]}</span>
                    <span className="state-tag">{label}</span>
                  </button>
                ))}
              </div>
            )}
            {!digest.data && (digest.failed ? <ErrorState what="the history" why={digest.failed} onRetry={digest.reload} /> : <Loading rows={6} />)}
            {digest.data && (
              <p className="digest-about">
                {digest.data.away ? "Since you left, " : "Since "}
                {dayLabel(digest.data.since_ms)} {clockOf(digest.data.since_ms)} ({age(digest.data.since_ms, digest.data.until_ms)} ago)
                {list && list.some((t) => t.kind === "session") ? `: ${shownCounts(list)}.` : "."}
              </p>
            )}
            {shown && shown.length === 0 && (
              <EmptyState
                title={tile ? `Nothing ${tile} in that time` : "Nothing happened in that time"}
                action={
                  tile
                    ? { label: "Show everything", run: () => setTile(null) }
                    : win !== "month"
                      ? { label: "Look back a month", run: () => setWin("month") }
                      : { label: "Start a session", run: () => (location.hash = "#/new") }
                }
              />
            )}
            {shown &&
              byDay(shown).map(([day, ts]) => (
                <section key={day}>
                  <h2 className="section-title">
                    {day} <span className="count">{ts.length}</span>
                  </h2>
                  <div className="list">
                    {ts.map((t) => {
                      const i = next();
                      return <TimelineRow key={t.key} t={t} picked={sel === i} />;
                    })}
                  </div>
                </section>
              ))}
          </>
        )}
      </main>
    </div>
  );
}

function TimelineRow({ t, picked }: { t: TimelineEntry; picked: boolean }) {
  const [busy, setBusy] = useState(false);
  if (t.kind === "supervisor") {
    const s = t.s;
    return (
      <div className={`row digest-row hist-row hist-slot ${picked ? "picked" : ""}`}>
        <span className="state-tag hist-by">{s.policy !== undefined ? `policy #${s.policy}` : "supervisor"}</span>
        <span className="row-main">
          <span className="row-name">
            {s.pr ? (
              <a href={s.pr} target="_blank" rel="noreferrer">
                {s.said}
              </a>
            ) : (
              s.said
            )}
          </span>
          <span className="row-sub">
            {base(s.project)} · {clockOf(t.at)}
          </span>
        </span>
      </div>
    );
  }
  const e = t.e;
  const sub = [`${e.agent} in ${e.repo}`, spanOf(e), e.branch].filter(Boolean).join(" · ");
  const p = t.past;
  const note = p?.cannot ? `Can't resume: ${p.cannot}` : p?.regrow ? `resuming adds its worktree back on ${p.regrow}` : null;
  const body = (
    <>
      <span className={`state-tag outcome-${t.outcome}`}>{shownLabel(t.outcome)}</span>
      <span className="row-main">
        <span className="row-name">{e.what}</span>
        <span className="digest-detail">{e.detail}</span>
        <span className="row-sub">{sub}</span>
        {e.summary && <span className="digest-said">{e.summary}</span>}
        {note && <span className="search-gone">{note}</span>}
      </span>
    </>
  );
  // A session still running opens; one that ended resumes, if it can.
  if (e.session !== undefined)
    return (
      <button className={`row digest-row hist-row hist-slot ${picked ? "picked" : ""}`} onClick={() => (location.hash = `#/s/${e.session}`)}>
        {body}
      </button>
    );
  return (
    <div className={`row digest-row hist-row hist-slot ${picked ? "picked" : ""}`}>
      {body}
      {p && !p.cannot && (
        <button
          className="ghost hist-resume"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            await resumePast(p);
            setBusy(false);
          }}
        >
          {busy ? "Resuming…" : "Resume"}
        </button>
      )}
    </div>
  );
}

type Search = { results: SearchResults | null; busy: boolean; failed: string | null; retry: () => void };

/** Searches session history for `query` from `since`, once typing settles. */
function useSearch(query: string, since: number | null): Search {
  const status = useApp((s) => s.status);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [tries, setTries] = useState(0);
  useEffect(() => {
    if (status !== "open" || terms(query).length === 0) {
      setResults(null);
      setFailed(null);
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      setBusy(true);
      setFailed(null);
      searchHistory(query, since).then(
        (r) => {
          if (!live) return;
          setResults(r);
          setBusy(false);
        },
        (e: Error) => {
          if (!live) return;
          setFailed(e.message);
          setBusy(false);
        },
      );
    }, SETTLE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [status, query, since, tries]);
  // Results answer the query they were asked with; a newer one waits for its own.
  const current = results !== null && results.query === query ? results : null;
  return { results: current, busy, failed, retry: () => setTries((n) => n + 1) };
}

function SearchResultsList({
  search,
  words,
  next,
  sel,
  onWider,
}: {
  search: Search;
  words: string[];
  next: () => number;
  sel: number;
  onWider: (() => void) | null;
}) {
  const { results, busy, failed, retry } = search;
  if (failed) return <ErrorState what="the search" why={failed} onRetry={retry} />;
  if (results === null || busy) return <div className="empty">Searching…</div>;
  if (results.total === 0)
    return <EmptyState title="No session in that time holds every word" action={onWider ? { label: "Search a month back", run: onWider } : undefined} />;
  return (
    <>
      <p className="digest-about">
        {results.total > results.hits.length
          ? `${results.total} sessions; the newest ${results.hits.length}.`
          : `${results.total} ${results.total === 1 ? "session" : "sessions"}.`}
      </p>
      <div className="list">
        {results.hits.map((h) => {
          const i = next();
          return <HitRow key={h.key} hit={h} words={words} picked={sel === i} />;
        })}
      </div>
    </>
  );
}

function HitRow({ hit, words, picked }: { hit: SearchHit; words: string[]; picked: boolean }) {
  const [starting, setStarting] = useState(false);
  const state = hit.session !== undefined ? "running" : hit.ended_ms !== undefined ? "ended" : "closed";
  const sub = [hit.repo, hit.agent, `${dayLabel(hit.at_ms)} ${clockOf(hit.at_ms)}`, state].join(" · ");
  const canResume = hit.session === undefined && hit.resume !== undefined && !hit.gone && hit.cwd !== undefined;

  const start = async () => {
    if (!hit.resume || !hit.cwd) return;
    setStarting(true);
    try {
      const id = await spawn(hit.resume, hit.cwd, null);
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Couldn't resume it: ${(e as Error).message}`);
      setStarting(false);
    }
  };

  const body = (
    <span className="row-main">
      <span className="row-name">{hit.what}</span>
      <span className="row-sub">{sub}</span>
      <span className="search-said">
        <span className="search-field">{fieldLabel(hit.field)}</span>{" "}
        {marks(hit.snippet, words).map((m, i) => (m.hit ? <mark key={i}>{m.text}</mark> : <span key={i}>{m.text}</span>))}
      </span>
      {hit.gone && hit.session === undefined && (
        <span className="search-gone">Its folder is gone ({hit.cwd}), so it can't be resumed from here.</span>
      )}
    </span>
  );
  if (hit.session !== undefined) {
    return (
      <button className={`row digest-row hist-row hist-slot ${picked ? "picked" : ""}`} onClick={() => (location.hash = `#/s/${hit.session}`)}>
        {body}
      </button>
    );
  }
  return (
    <div className={`row digest-row hist-row hist-slot ${picked ? "picked" : ""}`}>
      {body}
      {canResume && (
        <button className="ghost hist-resume" disabled={starting} onClick={() => void start()}>
          {starting ? "Starting…" : "Resume"}
        </button>
      )}
    </div>
  );
}
