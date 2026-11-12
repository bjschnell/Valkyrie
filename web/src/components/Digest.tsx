import { useEffect, useState } from "react";
import type { Digest, DigestEntry, DigestGroup } from "../proto";
import { counts, dayLabel, clockOf, groupDigest, outcomeLabel, spanOf, windowStart, type Window } from "../lib/digest";
import { age } from "../lib/time";
import { awayDigest, toast, useApp } from "../store";
import { Back } from "./icons";

const home = () => {
  location.hash = "#/";
};

const WINDOWS: [Window, string][] = [
  ["away", "since you left"],
  ["12h", "12 hours"],
  ["today", "today"],
  ["3d", "3 days"],
];

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

/**
 * What every agent did while you were gone (backlog #105): a line a session,
 * grouped by repository or day, with where its work ended up. Read-only; a
 * session still running opens on a tap. Copy gives the same as markdown, to
 * paste as a standup.
 */
export function DigestView() {
  const status = useApp((s) => s.status);
  const [win, setWin] = useState<Window>("away");
  const [by, setBy] = useState<DigestGroup>("repo");
  const [digest, setDigest] = useState<Digest | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    if (status !== "open") return;
    let live = true;
    awayDigest(windowStart(win, Date.now()), by).then(
      (d) => {
        if (!live) return;
        setDigest(d);
        setFailed(null);
      },
      (e: Error) => live && setFailed(e.message),
    );
    return () => {
      live = false;
    };
  }, [status, win, by]);

  // q or Esc goes back, when you aren't typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        home();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onCopy = async () => {
    if (!digest) return;
    toast((await copy(digest.markdown)) ? "Copied as markdown." : "Couldn't copy it here.");
  };

  const groups = digest ? groupDigest(digest.entries, by) : [];
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Away digest</div>
        </div>
        <button className="ghost digest-copy" disabled={!digest || digest.entries.length === 0} onClick={onCopy}>
          Copy
        </button>
      </header>

      <main className="inbox">
        <div className="kind-pick" role="radiogroup" aria-label="Since">
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
        <div className="kind-pick" role="radiogroup" aria-label="Group by">
          {(["repo", "day"] as const).map((g) => (
            <button
              key={g}
              role="radio"
              aria-checked={by === g}
              className={`kind-chip ${by === g ? "on" : ""}`}
              onClick={() => setBy(g)}
            >
              by {g}
            </button>
          ))}
        </div>
        {digest === null && <div className="empty">{failed ? `Couldn't load the digest: ${failed}` : "Loading…"}</div>}
        {digest !== null && (
          <p className="digest-about">
            {digest.away ? "Since you left, " : "Since "}
            {dayLabel(digest.since_ms)} {clockOf(digest.since_ms)} ({age(digest.since_ms, digest.until_ms)} ago)
            {digest.entries.length > 0 ? `: ${counts(digest.entries)}.` : "."}
          </p>
        )}
        {digest !== null && digest.entries.length === 0 && (
          <div className="empty">No agent did anything in that time.</div>
        )}
        {groups.map(([group, es]) => (
          <section key={group}>
            <h2 className="section-title">
              {group} <span className="count">{es.length}</span>
            </h2>
            <div className="list">
              {es.map((e) => (
                <DigestRow key={e.key} e={e} by={by} />
              ))}
            </div>
          </section>
        ))}
      </main>
    </div>
  );
}

function DigestRow({ e, by }: { e: DigestEntry; by: DigestGroup }) {
  const where = by === "day" ? `${e.agent} in ${e.repo}` : e.agent;
  const sub = [where, spanOf(e), e.session !== undefined ? "running" : null].filter(Boolean).join(" · ");
  const body = (
    <>
      <span className={`kind-chip outcome-${e.outcome}`}>{outcomeLabel(e.outcome)}</span>
      <span className="row-main">
        <span className="row-name">{e.what}</span>
        <span className="digest-detail">{e.detail}</span>
        <span className="row-sub">{sub}</span>
        {e.summary && <span className="digest-said">{e.summary}</span>}
      </span>
    </>
  );
  // A session still running opens; the rest are history.
  return e.session !== undefined ? (
    <button className="row digest-row" onClick={() => (location.hash = `#/s/${e.session}`)}>
      {body}
    </button>
  ) : (
    <div className="row digest-row">{body}</div>
  );
}
