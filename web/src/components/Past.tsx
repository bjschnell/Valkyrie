import { useState } from "react";
import { HistoryTabs } from "./Nav";
import type { PastSession } from "../proto";
import { clockOf, dayLabel, outcomeLabel } from "../lib/digest";
import { pastSessions, resume, toast } from "../store";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back } from "./icons";
import { usePageKeys } from "../lib/pageKeys";

const home = () => {
  location.hash = "#/";
};

/**
 * Past sessions (backlog #150): the agent sessions of the last weeks that have
 * ended, newest first. A tap resumes one on its own conversation, in a new tab,
 * adding its worktree back if that was taken away, and opens it.
 */
export function PastView() {
  const { data: list, failed, reload } = useCached("past", pastSessions);
  const [busy, setBusy] = useState<string | null>(null);

  // q or Esc goes back, when you aren't typing.
  usePageKeys("past", { q: home, Escape: home });

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
          <h1 className="title-name">Past sessions</h1>
        </div>
      </header>
      <HistoryTabs />

      <main className="inbox">
        {list === null && (failed ? <ErrorState what="past sessions" why={failed} onRetry={reload} /> : <Loading rows={6} />)}
        {list !== null && list.length === 0 && (
          <EmptyState
            title="No agent sessions have ended in the last 31 days"
            hint="Sessions you finish or close show up here, to pick up again."
            action={{ label: "Start a session", run: () => (location.hash = "#/new") }}
          />
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
      <span className={`state-tag outcome-${p.outcome}`}>{busy ? "resuming…" : outcomeLabel(p.outcome)}</span>
      <span className="row-main">
        <span className="row-name">{p.what}</span>
        <span className="row-sub">{sub}</span>
        <span className="digest-detail">{note}</span>
      </span>
    </button>
  );
}
