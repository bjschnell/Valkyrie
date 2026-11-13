import { useEffect, useState } from "react";
import type { PastSession } from "../proto";
import { clockOf, dayLabel, outcomeLabel } from "../lib/digest";
import { pastSessions, resume, toast, useApp } from "../store";
import { Back } from "./icons";

const home = () => {
  location.hash = "#/";
};

/**
 * Past sessions (backlog #150): the agent sessions of the last weeks that have
 * ended, newest first. A tap resumes one on its own conversation, in a new tab,
 * adding its worktree back if that was taken away, and opens it.
 */
export function PastView() {
  const status = useApp((s) => s.status);
  const [list, setList] = useState<PastSession[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    if (status !== "open") return;
    let live = true;
    pastSessions().then(
      (l) => {
        if (!live) return;
        setList(l);
        setFailed(null);
      },
      (e: Error) => live && setFailed(e.message),
    );
    return () => {
      live = false;
    };
  }, [status]);

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

  const onResume = async (p: PastSession) => {
    if (busy !== null) return;
    if (p.cannot) {
      toast(`Can't resume it: ${p.cannot}`);
      return;
    }
    setBusy(p.key);
    try {
      const id = await resume(p.key);
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Resume failed: ${(e as Error).message}`);
      setBusy(null);
    }
  };

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Past sessions</div>
        </div>
      </header>

      <main className="inbox">
        {list === null && <div className="empty">{failed ? `Couldn't load past sessions: ${failed}` : "Loading…"}</div>}
        {list !== null && list.length === 0 && (
          <div className="empty">No agent sessions have ended in the last 31 days.</div>
        )}
        {list !== null && list.length > 0 && (
          <p className="digest-about">Agent sessions that ended, newest first. Tap one to pick it up where it left off.</p>
        )}
        <div className="list">
          {list?.map((p) => (
            <PastRow key={p.key} p={p} busy={busy === p.key} onResume={() => onResume(p)} />
          ))}
        </div>
      </main>
    </div>
  );
}

function PastRow({ p, busy, onResume }: { p: PastSession; busy: boolean; onResume: () => void }) {
  const at = p.ended_ms ?? p.last_ms;
  const sub = [`${dayLabel(at)} ${clockOf(at)}`, `${p.agent} in ${p.repo}`, p.branch].filter(Boolean).join(" · ");
  const note = p.cannot
    ? `Can't resume: ${p.cannot}`
    : p.regrow
      ? `${p.detail} · adds its worktree back on ${p.regrow}`
      : p.detail;
  return (
    <button
      className={`row digest-row ${p.cannot ? "past-cannot" : ""}`}
      aria-disabled={p.cannot !== undefined}
      disabled={busy}
      onClick={onResume}
    >
      <span className={`kind-chip outcome-${p.outcome}`}>{busy ? "resuming…" : outcomeLabel(p.outcome)}</span>
      <span className="row-main">
        <span className="row-name">{p.what}</span>
        <span className="row-sub">{sub}</span>
        <span className="digest-detail">{note}</span>
      </span>
    </button>
  );
}
