import { useEffect, useRef, useState } from "react";
import { HistoryTabs } from "./Nav";
import type { WatchedPr } from "../proto";
import { base, itemNo } from "../lib/backlog";
import { watchLine } from "../lib/supervisor";
import { age } from "../lib/time";
import { supervisorLog, supervisorSession, supervisorWatch, toast, useApp } from "../store";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back } from "./icons";
import { usePageKeys } from "../lib/pageKeys";

const home = () => {
  location.hash = "#/";
};

/** The log as last fetched, refetched while `key` changes (an answer lands). */
function useLog(key: unknown) {
  const { data: log, failed, reload } = useCached("supervisor-log", supervisorLog);
  const first = useRef(true);
  useEffect(() => {
    if (first.current) first.current = false;
    else void reload();
  }, [key, reload]);
  return { log, failed, reload };
}

/**
 * The supervisor page (backlog #180): what it watches and what it did: steps you
 * answered, steps policies took without asking, build orders you answered. What
 * it waits on you for is in the Inbox's queue (#251).
 */
export function SupervisorView() {
  const now = useApp((s) => s.now);
  // An answer in the Inbox changes what it did: read it again when one lands.
  const pending = useApp((s) => s.launches.length + s.orders.length + s.queue.filter((q) => q.flag?.step).length);
  const { log, failed, reload } = useLog(pending);
  const [opening, setOpening] = useState(false);
  const openSession = async () => {
    if (opening) return;
    setOpening(true);
    try {
      const id = await supervisorSession();
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Supervisor session: ${(e as Error).message}`);
      setOpening(false);
    }
  };

  // q or Esc goes back, when you aren't typing; c opens the supervisor session.
  usePageKeys("supervisor", { q: home, Escape: home, c: () => void openSession() });

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Supervisor</h1>
          <div className="title-sub">{log && !log.on ? "off: valk supervise on turns it back on" : "watches PRs and the backlog after agents stop"}</div>
        </div>
        <button className="ghost" disabled={opening || log?.on === false} onClick={() => void openSession()} title="One agent that reads every session and prompts the idle ones (c)">
          Session
        </button>
      </header>
      <HistoryTabs />
      <main className="inbox supervisor">
        <Watching refresh={pending} />
        <section>
          <h2 className="section-title">
            What it did <span className="count">{log?.entries.length ?? ""}</span>
          </h2>
          {failed && !log && <ErrorState what="what it did" why={failed} onRetry={reload} />}
          {!failed && !log && <Loading rows={3} />}
          {log && log.entries.length === 0 && (
            <EmptyState
              title="Nothing in the last 90 days"
              hint="Steps it takes after an agent stops show up here."
              action={{ label: "Open the backlog", run: () => (location.hash = "#/backlog") }}
            />
          )}
          {log && log.entries.length > 0 && (
            <ol className="supervisor-log">
              {log.entries.map((e, i) => (
                <li key={`${e.at}:${i}`} className={e.policy !== undefined ? "by-policy" : ""}>
                  <span className="supervisor-when">{age(e.at * 1000, now)}</span>
                  <span className="supervisor-said">
                    {e.pr ? (
                      <a href={e.pr} target="_blank" rel="noreferrer">
                        {e.said}
                      </a>
                    ) : (
                      e.said
                    )}
                  </span>
                  <span className="supervisor-repo">{base(e.project)}</span>
                </li>
              ))}
            </ol>
          )}
        </section>
      </main>
    </div>
  );
}

const STATE_LABEL: Record<WatchedPr["state"], string> = {
  green: "green",
  pending: "pending",
  red: "red",
  conflicting: "conflicting",
  unknown: "unread",
};

/**
 * What the supervisor watches (backlog #181): each open PR its poll read, in the
 * order to merge each repository's (backlog #215), with why and whom it collides
 * with, and when the poll reads them next. Read again
 * every minute while the page is open, since the poll runs every five.
 */
function Watching({ refresh }: { refresh: number }) {
  const items = useApp((s) => s.items);
  const status = useApp((s) => s.status);
  const now = useApp((s) => s.now);
  const { data: watch, failed, reload } = useCached("supervisor-watch", supervisorWatch);
  useEffect(() => {
    if (status !== "open") return;
    if (refresh) void reload();
    const every = setInterval(() => void reload(), 60_000);
    return () => clearInterval(every);
  }, [status, refresh, reload]);
  return (
    <section>
      <h2 className="section-title">
        Watching <span className="count">{watch?.prs.length ?? ""}</span>
      </h2>
      {watch && <div className="supervisor-watch-line">{watchLine(watch, now)}</div>}
      {failed && !watch && <ErrorState what="the pull requests it watches" why={failed} onRetry={reload} />}
      {!failed && !watch && <Loading rows={2} />}
      {watch && watch.on && watch.prs.length === 0 && (
        <EmptyState
          title="No open pull request of yours"
          hint="It watches the ones you open."
          action={{ label: "Open the backlog", run: () => (location.hash = "#/backlog") }}
        />
      )}
      {watch && watch.prs.length > 0 && (
        <ol className="supervisor-log supervisor-watch">
          {watch.prs.map((p) => (
            <li key={p.url}>
              <span className={`supervisor-state state-${p.state}`}>
                {p.order !== undefined && <span className="supervisor-order">{p.order}.</span>}
                {STATE_LABEL[p.state]}
              </span>
              <span className="supervisor-said">
                <a href={p.url} target="_blank" rel="noreferrer">
                  {p.detail}
                </a>
                {p.merge_note && <span className="supervisor-merge-note">{p.merge_note}</span>}
              </span>
              <span className="supervisor-repo">
                {p.item !== undefined ? `#${itemNo(items, p.item)}` : p.branch} · {base(p.project)}
              </span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
