import { useEffect, useRef, useState } from "react";
import { STATE_LABEL, program, type QueueItem, type SessionId, type SessionInfo } from "../proto";
import { isNegative, plainYes, weakSummary, type Choice } from "../lib/choices";
import { age } from "../lib/time";
import { answer, answerAll, kill, useApp } from "../store";
import { useLook } from "../lib/theme";
import { Chevron, Gear, List, Plus } from "./icons";
import { ConnectionPill, StateIcon } from "./bits";
import { Proposals } from "./Decisions";
import { NotifyHint } from "./Notify";
import { Rain } from "./Rain";
import { Mark, Wordmark } from "./Logo";

const go = (id: number) => {
  location.hash = `#/s/${id}`;
};

/** Home: who needs you, with the answers right on the card, then every session. */
export function Inbox() {
  const queue = useApp((s) => s.queue);
  const sessions = useApp((s) => s.sessions);
  const openItems = useApp((s) => s.items.filter((i) => i.status === "open").length);
  const look = useLook();
  const working = sessions.filter((s) => s.exited === null && s.status.state === "working").length;
  const [sel, setSel] = useState(-1);
  // The one row swiped open to show its Close button.
  const [swiped, setSwiped] = useState<SessionId | null>(null);
  // What j/k move through: the cards, then the session rows.
  const order = [...queue.map((q) => q.session), ...sessions.map((s) => s.id)];
  const selected = order[sel] ?? null;

  // n starts a session; j/k pick a card or row, Enter opens it, r reviews its
  // changes, b opens the backlog, `,` opens settings.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      const pick = (i: number) => {
        const next = Math.max(0, Math.min(order.length - 1, i));
        setSel(next);
        document.querySelectorAll<HTMLElement>("[data-pick]")[next]?.scrollIntoView({ block: "nearest" });
      };
      const actions: Record<string, () => void> = {
        n: () => (location.hash = "#/new"),
        b: () => (location.hash = "#/backlog"),
        ",": () => (location.hash = "#/settings"),
        j: () => pick(sel + 1),
        k: () => pick(sel - 1),
        Enter: () => selected !== null && go(selected),
        o: () => selected !== null && go(selected),
        r: () => selected !== null && (location.hash = `#/s/${selected}/review`),
      };
      const action = actions[e.key];
      if (action) {
        e.preventDefault();
        action();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="page">
      {look !== "default" && <Rain key={look} />}
      <header className="topbar">
        <div className="brand">
          <Mark size={26} />
          <Wordmark />
        </div>
        <ConnectionPill />
        <button className="icon-btn" aria-label="Backlog" title="Backlog" onClick={() => (location.hash = "#/backlog")}>
          <List />
          {openItems > 0 && <span className="icon-badge">{openItems}</span>}
        </button>
        <button className="icon-btn" aria-label="Settings" title="Settings" onClick={() => (location.hash = "#/settings")}>
          <Gear />
        </button>
        <button className="icon-btn" aria-label="New session" onClick={() => (location.hash = "#/new")}>
          <Plus />
        </button>
      </header>

      <main className="inbox">
        <NotifyHint />
        <AllowAll />
        <Proposals />
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
              {queue.map((item, i) => (
                <QueueCard
                  key={item.session}
                  item={item}
                  info={sessions.find((s) => s.id === item.session)}
                  picked={sel === i}
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
              <SessionRow
                key={s.id}
                s={s}
                n={i + 1}
                picked={sel === queue.length + i}
                swiped={swiped === s.id}
                onSwipe={(open) => setSwiped((was) => (open ? s.id : was === s.id ? null : was))}
              />
            ))}
            {sessions.length === 0 && <div className="empty">No sessions yet.</div>}
          </div>
        </section>
      </main>
    </div>
  );
}

/**
 * Several prompts waiting at once, each with a plain yes: allow them in one go,
 * after seeing what each one is. Only the first choice, and only when it is a
 * plain yes, never "don't ask again" (see `plainYes`).
 */
function AllowAll() {
  const queue = useApp((s) => s.queue);
  const prompts = useApp((s) => s.prompts);
  const [open, setOpen] = useState(false);
  const [skip, setSkip] = useState<Set<number>>(new Set());
  const ready = queue
    .filter((q) => q.status.state === "needs_input")
    .map((q) => {
      const choices = prompts[q.session]?.choices ?? [];
      const about = weakSummary(q.status.summary) ? (prompts[q.session]?.context ?? q.status.summary) : q.status.summary;
      return { item: q, choices, yes: plainYes(choices), about };
    })
    .filter((x): x is { item: QueueItem; choices: Choice[]; yes: Choice; about: string | null } => x.yes !== null);
  if (ready.length < 2) return null;
  const chosen = ready.filter((x) => !skip.has(x.item.session));

  if (!open) {
    return (
      <button className="allow-bar" onClick={() => setOpen(true)}>
        <span>
          <strong>{ready.length} prompts</strong> waiting for a yes
        </span>
        <span className="allow-go">Allow all…</span>
      </button>
    );
  }
  return (
    <div className="allow-panel">
      <div className="allow-title">Allow these?</div>
      {ready.map(({ item, yes, about }) => (
        <label key={item.session} className="allow-row">
          <input
            type="checkbox"
            checked={!skip.has(item.session)}
            onChange={() =>
              setSkip((was) => {
                const next = new Set(was);
                if (next.has(item.session)) next.delete(item.session);
                else next.add(item.session);
                return next;
              })
            }
          />
          <span className="allow-what">
            <span className="allow-name">{item.name}</span>
            <span className="allow-summary">{about ?? "needs input"}</span>
          </span>
          <span className="allow-choice">{yes.label}</span>
        </label>
      ))}
      <div className="allow-actions">
        <button className="ghost" onClick={() => setOpen(false)}>
          Cancel
        </button>
        <button
          className="primary"
          disabled={chosen.length === 0}
          onClick={() => {
            setOpen(false);
            void answerAll(chosen.map((x) => ({ session: x.item.session, choices: x.choices, choice: x.yes })));
          }}
        >
          Allow {chosen.length}
        </button>
      </div>
    </div>
  );
}

function QueueCard({ item, info, picked }: { item: QueueItem; info?: SessionInfo; picked: boolean }) {
  const now = useApp((s) => s.now);
  const prompt = useApp((s) => s.prompts[item.session]);
  const s = item.status;
  const choices = prompt?.choices ?? [];
  return (
    <div className={`card-slot ${picked ? "picked" : ""}`} data-pick>
      <article className={`card state-${s.state}`}>
        <button className="card-head" onClick={() => go(item.session)}>
          <StateIcon state={s.state} />
          <span className="card-name">{item.name}</span>
          <span className="card-prog">{info ? program(info) : s.agent}</span>
          <span className="card-age">{age(s.since_ms, now)}</span>
        </button>
        <div className="card-state">{STATE_LABEL[s.state]}</div>
        {weakSummary(s.summary) && prompt?.context ? (
          <p className="card-summary">{prompt.context}</p>
        ) : (
          s.summary && <p className="card-summary">{s.summary}</p>
        )}
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
          {s.state === "review_ready" && (
            <button className="ghost" onClick={() => (location.hash = `#/s/${item.session}/review`)}>
              Review changes
            </button>
          )}
        </div>
      </article>
    </div>
  );
}

/** How far a row slides to show its Close button. */
const REVEAL = 88;

/**
 * A session in the list. Swiped left on a touch screen it shows Close, which
 * ends the session; tapping the row again slides it back.
 */
function SessionRow({
  s,
  n,
  picked,
  swiped,
  onSwipe,
}: {
  s: SessionInfo;
  n: number;
  picked: boolean;
  swiped: boolean;
  onSwipe: (open: boolean) => void;
}) {
  const now = useApp((st) => st.now);
  const queued = useApp((st) => st.queue.some((q) => q.session === s.id));
  const state = s.exited !== null ? "exited" : s.status.state;
  // Where the row is while a finger drags it; null when it rests.
  const [drag, setDrag] = useState<number | null>(null);
  // Null until the finger has moved far enough to tell a swipe from a scroll.
  const gesture = useRef<{ x: number; y: number; sideways: boolean | null } | null>(null);
  // A swipe ends in a click on some browsers, which must not open the session.
  const swallow = useRef(false);
  const rest = swiped ? -REVEAL : 0;
  const x = drag ?? rest;

  const end = () => {
    const sideways = gesture.current?.sideways;
    gesture.current = null;
    if (!sideways || drag === null) return setDrag(null);
    swallow.current = true;
    setDrag(null);
    const open = drag < -REVEAL / 2;
    if (open && !swiped) navigator.vibrate?.(10);
    onSwipe(open);
  };

  return (
    <div className="row-slot" data-pick>
      <button className="row-close" tabIndex={swiped ? 0 : -1} aria-hidden={!swiped} onClick={() => void kill(s.id)}>
        {state === "exited" ? "Remove" : "Close"}
      </button>
      <button
        className={`row ${queued ? "queued" : ""} ${picked ? "picked" : ""} ${drag !== null ? "dragging" : ""}`}
        style={x ? { transform: `translateX(${x}px)` } : undefined}
        onPointerDown={(e) => {
          swallow.current = false;
          if (e.pointerType !== "mouse") gesture.current = { x: e.clientX, y: e.clientY, sideways: null };
        }}
        onPointerMove={(e) => {
          const g = gesture.current;
          if (!g) return;
          const dx = e.clientX - g.x;
          const dy = e.clientY - g.y;
          if (g.sideways === null) {
            if (Math.hypot(dx, dy) < 8) return;
            g.sideways = Math.abs(dx) > Math.abs(dy);
            // Up or down: the page scrolls, and this was never a swipe.
            if (!g.sideways) return void (gesture.current = null);
            e.currentTarget.setPointerCapture(e.pointerId);
          }
          setDrag(Math.max(-REVEAL * 1.4, Math.min(0, rest + dx)));
        }}
        onPointerUp={end}
        onPointerCancel={() => {
          gesture.current = null;
          setDrag(null);
        }}
        onClick={() => {
          if (swallow.current) return void (swallow.current = false);
          if (swiped) return onSwipe(false);
          go(s.id);
        }}
      >
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
    </div>
  );
}
