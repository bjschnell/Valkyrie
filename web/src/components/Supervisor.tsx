import { useEffect, useRef, useState } from "react";
import type { WatchedPr } from "../proto";
import { base, launchesOn, orderHref, orderSize } from "../lib/backlog";
import { stepChoices } from "../lib/step";
import { stripLine, waitingOnYou, watchLine, type Wait } from "../lib/supervisor";
import { age } from "../lib/time";
import { answerLaunch, decide, supervisorLog, supervisorWatch, takeStep, useApp } from "../store";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back, Chevron } from "./icons";
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

/** What waits on you, from what the app already holds. */
function useWaiting(): Wait[] {
  const queue = useApp((s) => s.queue);
  const launches = useApp((s) => s.launches);
  const orders = useApp((s) => s.orders);
  const proposals = useApp((s) => s.proposals);
  return waitingOnYou(queue, launches, orders, proposals);
}

/**
 * The home screen's strip (backlog #180): what waits on you and the latest thing
 * the supervisor did, a tap from the whole page. Nothing when there's nothing.
 */
export function SupervisorStrip() {
  const waits = useWaiting();
  const { log } = useLog(waits.length);
  const line = stripLine(waits.length, log?.entries[0]);
  if (!line) return null;
  return (
    <button className="supervisor-strip" onClick={() => (location.hash = "#/supervisor")}>
      <span className="supervisor-strip-name">Supervisor</span>
      <span className="supervisor-strip-line">{line}</span>
      <Chevron />
    </button>
  );
}

/**
 * The supervisor page (backlog #180): what it waits on you for, each answered in
 * place, and what it did: steps you answered, steps policies took without
 * asking, build orders you answered.
 */
export function SupervisorView() {
  const waits = useWaiting();
  const now = useApp((s) => s.now);
  const [answered, setAnswered] = useState(0);
  const { log, failed, reload } = useLog(`${waits.length}:${answered}`);
  const done = () => setAnswered((n) => n + 1);

  // q or Esc goes back, when you aren't typing.
  usePageKeys("supervisor", { q: home, Escape: home });

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
      </header>
      <main className="inbox supervisor">
        <section>
          <h2 className="section-title">
            Waiting on you <span className="count">{waits.length}</span>
          </h2>
          {waits.length === 0 ? (
            <div className="calm">
              <div className="calm-title">Nothing waits on you</div>
              <div className="calm-sub">No flagged PR, ready item, build order or policy to answer.</div>
            </div>
          ) : (
            <div className="cards">
              {waits.map((w) => (
                <WaitCard key={waitKey(w)} w={w} onDone={done} />
              ))}
            </div>
          )}
        </section>
        <Watching refresh={answered} />
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

const waitKey = (w: Wait) =>
  w.kind === "flag"
    ? `f${w.item.session}`
    : w.kind === "launch"
      ? `l${w.offer.project}:${w.offer.id}`
      : w.kind === "order"
        ? `o${w.order.project}:${w.order.id}`
        : `p${w.decision.project}#${w.decision.id}`;

/** One thing waiting, with what answers it. */
function WaitCard({ w, onDone }: { w: Wait; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await f();
      onDone();
    } finally {
      setBusy(false);
    }
  };
  const card = (tag: string, title: string, sub: string, buttons: React.ReactNode) => (
    <section className="supervisor-card">
      <div className="supervisor-card-head">
        <span className="supervisor-tag">{tag}</span>
        <span className="supervisor-card-title">{title}</span>
      </div>
      {sub && <div className="supervisor-card-sub">{sub}</div>}
      <div className="order-actions">{buttons}</div>
    </section>
  );
  switch (w.kind) {
    case "flag": {
      const flag = w.item.flag!;
      return card(
        "PR",
        `${w.item.name}: ${flag.reason}`,
        flag.step?.why ?? "",
        stepChoices(flag).map((c) => (
          <button
            key={c.action}
            className={c.primary ? "primary" : "ghost"}
            disabled={busy}
            onClick={() => void act(() => takeStep(w.item.session, flag.seq, c.action))}
          >
            {c.label}
          </button>
        )),
      );
    }
    case "launch":
      return card("ready", `#${w.offer.item} ${w.offer.title}`, `${w.offer.why} · on ${launchesOn(w.offer)} · ${base(w.offer.project)}`, [
        <button key="leave" className="ghost" disabled={busy} onClick={() => void act(() => answerLaunch(w.offer, false))}>
          Leave
        </button>,
        <button key="start" className="primary" disabled={busy} onClick={() => void act(() => answerLaunch(w.offer, true))}>
          Start
        </button>,
      ]);
    case "order":
      return card("order", `Build order for ${base(w.order.project)}`, `${orderSize(w.order)} · ${w.order.trigger}`, [
        <button key="review" className="primary" onClick={() => (location.hash = orderHref(w.order.project))}>
          Review
        </button>,
      ]);
    case "policy":
      return card("policy", `#${w.decision.id} ${w.decision.title}`, `A policy for ${base(w.decision.project)}: once accepted, it runs without asking.`, [
        <button key="reject" className="ghost" disabled={busy} onClick={() => decide(w.decision, "reject", onDone)}>
          Reject
        </button>,
        <button key="accept" className="primary" disabled={busy} onClick={() => decide(w.decision, "accept", onDone)}>
          Accept
        </button>,
      ]);
  }
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
                {p.item !== undefined ? `#${p.item}` : p.branch} · {base(p.project)}
              </span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
