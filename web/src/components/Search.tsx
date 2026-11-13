import { useEffect, useRef, useState } from "react";
import type { SearchHit, SearchResults } from "../proto";
import { fieldLabel, marks, terms, windowStart, WINDOWS, type SearchWindow } from "../lib/search";
import { clockOf, dayLabel } from "../lib/digest";
import { searchHistory, spawn, toast, useApp } from "../store";
import { Back } from "./icons";

const home = () => {
  location.hash = "#/";
};

/** How long to wait for typing to stop before asking the daemon. */
const SETTLE_MS = 250;

/**
 * Find the session where you said or did something (backlog #106): every word
 * in your prompts, the agents' closing replies, the files they wrote, or what
 * they were about. A running session opens on a tap; a closed one resumes its
 * conversation in a new session where it worked.
 */
export function SearchView() {
  const status = useApp((s) => s.status);
  const [query, setQuery] = useState("");
  const [win, setWin] = useState<SearchWindow>("all");
  // With the window they were asked for: a new window waits for its own.
  const [results, setResults] = useState<{ win: SearchWindow; r: SearchResults } | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => input.current?.focus(), []);

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
      searchHistory(query, windowStart(win, Date.now())).then(
        (r) => {
          if (!live) return;
          setResults({ win, r });
          setFailed(null);
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
  }, [status, query, win]);

  // q or Esc goes back, when you aren't typing; Esc in the box leaves it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if ((e.target as HTMLElement).closest("textarea, input")) {
        if (e.key === "Escape") (e.target as HTMLElement).blur();
        return;
      }
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        home();
      } else if (e.key === "/") {
        e.preventDefault();
        input.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const words = terms(query);
  // Results answer the query they were asked with; a newer one waits for its own.
  const shown = results !== null && results.r.query === query && results.win === win ? results.r : null;
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Search sessions</div>
        </div>
      </header>

      <main className="inbox">
        <div className="field search-box">
          <input
            ref={input}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder='Words you or an agent said, a file, a branch, "a phrase"'
            aria-label="Search session history"
            enterKeyHint="search"
            spellCheck={false}
            autoCapitalize="off"
          />
        </div>
        <div className="kind-pick" role="radiogroup" aria-label="How far back">
          {WINDOWS.map(([w, label]) => (
            <button
              key={w}
              role="radio"
              aria-checked={win === w}
              className={`kind-chip ${win === w ? "on" : ""}`}
              onClick={() => setWin(w)}
            >
              {label}
            </button>
          ))}
        </div>
        {failed && <div className="empty">Couldn't search: {failed}</div>}
        {!failed && words.length === 0 && (
          <div className="empty">Find the session where you said or did something.</div>
        )}
        {!failed && words.length > 0 && (shown === null || busy) && <div className="empty">Searching…</div>}
        {!failed && shown !== null && !busy && (
          <p className="digest-about">
            {shown.total === 0
              ? "No session holds every word."
              : shown.total > shown.hits.length
                ? `${shown.total} sessions; the newest ${shown.hits.length}.`
                : `${shown.total} ${shown.total === 1 ? "session" : "sessions"}.`}
          </p>
        )}
        {!failed && shown !== null && !busy && shown.hits.length > 0 && (
          <div className="list">
            {shown.hits.map((h) => (
              <HitRow key={h.key} hit={h} words={words} />
            ))}
          </div>
        )}
      </main>
    </div>
  );
}

function HitRow({ hit, words }: { hit: SearchHit; words: string[] }) {
  const [starting, setStarting] = useState(false);
  const state = hit.session !== undefined ? "running" : hit.ended_ms !== undefined ? "ended" : "closed";
  const sub = [hit.repo, hit.agent, `${dayLabel(hit.at_ms)} ${clockOf(hit.at_ms)}`, state].join(" · ");
  const canResume = hit.session === undefined && hit.resume !== undefined && !hit.gone && hit.cwd !== undefined;

  const resume = async () => {
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
      <button className="row digest-row" onClick={() => (location.hash = `#/s/${hit.session}`)}>
        {body}
      </button>
    );
  }
  return (
    <div className="row digest-row">
      {body}
      {canResume && (
        <button className="ghost search-resume" disabled={starting} onClick={resume}>
          {starting ? "Starting…" : "Resume"}
        </button>
      )}
    </div>
  );
}
