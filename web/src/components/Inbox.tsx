import { useRef, useState } from "react";
import { STATE_LABEL, program, type QueueItem, type SessionInfo } from "../proto";
import { isNegative } from "../lib/choices";
import { age } from "../lib/time";
import { answer, markSeen, useApp } from "../store";
import { Chevron } from "./icons";
import { ConnectionPill, StateIcon } from "./bits";

const go = (id: number) => {
  location.hash = `#/s/${id}`;
};

/** Home: who needs you, with the answers right on the card, then every session. */
export function Inbox() {
  const queue = useApp((s) => s.queue);
  const sessions = useApp((s) => s.sessions);
  const working = sessions.filter((s) => s.exited === null && s.status.state === "working").length;

  return (
    <div className="page">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">◆</span> Valkyrie
        </div>
        <ConnectionPill />
      </header>

      <main className="inbox">
        <section>
          <h2 className="section-title">
            Needs you <span className="count">{queue.length}</span>
          </h2>
          {queue.length === 0 ? (
            <div className="calm">
              <div className="calm-mark">✓</div>
              <div className="calm-title">All clear</div>
              <div className="calm-sub">
                {working ? `${working} working · nothing needs you` : "Nothing needs you"}
              </div>
            </div>
          ) : (
            <div className="cards">
              {queue.map((item) => (
                <QueueCard
                  key={item.session}
                  item={item}
                  info={sessions.find((s) => s.id === item.session)}
                />
              ))}
            </div>
          )}
        </section>

        <section>
          <h2 className="section-title">
            Sessions <span className="count">{sessions.length}</span>
          </h2>
          <div className="list">
            {sessions.map((s, i) => (
              <SessionRow key={s.id} s={s} n={i + 1} />
            ))}
            {sessions.length === 0 && <div className="empty">No sessions yet.</div>}
          </div>
        </section>
      </main>
    </div>
  );
}

/** Swipe it aside to mark it seen. */
const SWIPE_PX = 90;

function QueueCard({ item, info }: { item: QueueItem; info?: SessionInfo }) {
  const now = useApp((s) => s.now);
  const prompt = useApp((s) => s.prompts[item.session]);
  const s = item.status;
  const [dx, setDx] = useState(0);
  const start = useRef<{ x: number; y: number } | null>(null);
  const swiping = useRef(false);

  const onTouchStart = (e: React.TouchEvent) => {
    start.current = { x: e.touches[0].clientX, y: e.touches[0].clientY };
    swiping.current = false;
  };
  const onTouchMove = (e: React.TouchEvent) => {
    if (!start.current) return;
    const x = e.touches[0].clientX - start.current.x;
    const y = e.touches[0].clientY - start.current.y;
    // Mostly sideways is a swipe; mostly down is the page scrolling.
    if (!swiping.current && Math.abs(x) > 12 && Math.abs(x) > Math.abs(y) * 1.5) swiping.current = true;
    if (swiping.current) setDx(x);
  };
  const onTouchEnd = () => {
    if (Math.abs(dx) > SWIPE_PX) {
      navigator.vibrate?.(10);
      setDx(dx > 0 ? 600 : -600);
      void markSeen(item);
    } else {
      setDx(0);
    }
    start.current = null;
  };

  const choices = prompt?.choices ?? [];
  return (
    <div className="card-slot">
      <div className="card-under" style={{ opacity: Math.min(1, Math.abs(dx) / SWIPE_PX) }}>
        seen
      </div>
      <article
        className={`card state-${s.state}`}
        style={{ transform: `translateX(${dx}px)`, transition: start.current ? "none" : undefined }}
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
      >
        <button className="card-head" onClick={() => go(item.session)}>
          <StateIcon state={s.state} />
          <span className="card-name">{item.name}</span>
          <span className="card-prog">{info ? program(info) : s.agent}</span>
          <span className="card-age">{age(s.since_ms, now)}</span>
        </button>
        <div className="card-state">{STATE_LABEL[s.state]}</div>
        {s.summary && <p className="card-summary">{s.summary}</p>}
        {choices.length > 0 && (
          <div className="choices">
            {choices.map((c, i) => (
              <button
                key={i}
                className={`choice ${isNegative(c.label) ? "no" : i === 0 ? "yes" : ""}`}
                onClick={() => void answer(item.session, choices, c)}
              >
                {c.n !== undefined && <span className="choice-n">{c.n}</span>}
                {c.label}
              </button>
            ))}
          </div>
        )}
        <div className="card-actions">
          <button className="ghost" onClick={() => go(item.session)}>
            Open
          </button>
          <button className="ghost" onClick={() => void markSeen(item)}>
            Seen
          </button>
        </div>
      </article>
    </div>
  );
}

function SessionRow({ s, n }: { s: SessionInfo; n: number }) {
  const now = useApp((st) => st.now);
  const queued = useApp((st) => st.queue.some((q) => q.session === s.id));
  const state = s.exited !== null ? "exited" : s.status.state;
  return (
    <button className={`row ${queued ? "queued" : ""}`} onClick={() => go(s.id)}>
      <span className="row-n">{n}</span>
      <StateIcon state={state} />
      <span className="row-main">
        <span className={`row-name state-text-${queued ? state : "plain"}`}>{s.name}</span>
        <span className="row-sub">
          {program(s)} · {state === "exited" ? `exited ${s.exited ?? ""}` : STATE_LABEL[state]}
          {s.status.summary ? ` · ${s.status.summary}` : ""}
        </span>
      </span>
      <span className="row-age">{age(s.status.since_ms, now)}</span>
      <Chevron />
    </button>
  );
}
