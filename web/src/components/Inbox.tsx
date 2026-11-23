import { HASH } from "../lib/routes";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import {
  STATE_LABEL,
  program,
  type QueueItem,
  type SessionId,
  type SessionInfo,
} from "../proto";
import { isNegative, plainYes, weakSummary, type Choice } from "../lib/choices";
import { answeringLabel } from "../lib/answering";
import { age } from "../lib/time";
import { answer, answerAll, kill, refreshUnreadDocs, takeStep, useApp } from "../store";
import { stepChoices } from "../lib/step";
import { hasRain, useLook } from "../lib/theme";
import { Chevron, Gear, Plus } from "./icons";
import { useDesktop, useWide } from "./Terminal";
import { Composer, focusComposer, ReadyList } from "./Composer";
import { ConnectionPill, StateIcon } from "./bits";
import { EmptyState } from "./states";
import { isRecord, RecordRow, useAttention } from "./Attention";
import { sessionKind } from "../lib/attention";
import { NotifyHint } from "./Notify";
import { QuotaMeter } from "./Quota";
import { Rain } from "./Rain";
import { Mark, Wordmark } from "./Logo";
import { usePageKeys } from "../lib/pageKeys";
import { TutorButton } from "./Tutor";

const go = (id: number) => {
  location.hash = `#/s/${id}`;
};

/** Home: who needs you, with the answers right on the card, then every session.
 * `home`: the pane beside it shows the home composer (#275). */
export function Inbox({ beside, pane, home = false }: { beside?: number | null; pane?: string | null; home?: boolean } = {}) {
  const queue = useApp((s) => s.queue);
  const sessions = useApp((s) => s.sessions);
  const status = useApp((s) => s.status);
  useEffect(() => {
    if (status !== "open") return;
    void refreshUnreadDocs();
    const every = setInterval(() => void refreshUnreadDocs(), 60_000);
    return () => clearInterval(every);
  }, [status]);
  const look = useLook();
  const desktop = useDesktop();
  // With a pane beside the list, New session is its top row and opens the composer there.
  const wide = useWide();
  const working = sessions.filter(
    (s) => s.exited === null && s.status.state === "working",
  ).length;
  const [sel, setSel] = useState(-1);
  // The record tapped open in Needs you; a picked one is open too.
  const [opened, setOpened] = useState<string | null>(null);
  // The one row swiped open to show its Close button.
  const [swiped, setSwiped] = useState<SessionId | null>(null);
  // Everything waiting on you, sessions (Ask, Stuck, Done) then records (Decide,
  // Filed), each drawn once: a waiting session as its card, the rest as a row
  // under Running or Quiet. A row keeps its number, which Alt+digit goes to.
  const needs = useAttention();
  const records = needs.filter(isRecord);
  const waiting = needs.flatMap((a) => (isRecord(a) ? [] : [a.item]));
  const queued = new Set(queue.map((q) => q.session));
  const rows = sessions.map((s, i) => ({ s, n: i + 1 })).filter(({ s }) => !queued.has(s.id));
  const running = rows.filter(({ s }) => s.exited === null && s.status.state === "working");
  const quiet = rows.filter(({ s }) => !(s.exited === null && s.status.state === "working"));
  // What j/k move through, in the order they're drawn: `new` for New session,
  // keys, `s<id>` for a session.
  const order = [...(wide ? ["new"] : []), ...needs.map((a) => a.key), ...[...running, ...quiet].map(({ s }) => `s${s.id}`)];
  const selected = order[sel] ?? null;

  // n starts a session; j/k pick a card or row, Enter opens it (or a record's
  // card), Esc closes that, r reviews a session's changes, b opens the backlog,
  // d the decisions, `,` opens settings.
  const pick = (i: number) => {
    const next = Math.max(0, Math.min(order.length - 1, i));
    setSel(next);
    document.querySelectorAll<HTMLElement>("[data-pick]")[next]?.scrollIntoView({ block: "nearest" });
  };
  const sessionOf = (key: string | null) => (key?.startsWith("s") ? Number(key.slice(1)) : null);
  const open = () => {
    if (selected === "new") return void (location.hash = "#/new");
    const id = sessionOf(selected);
    if (id !== null) return go(id);
    const rec = records.find((r) => r.key === selected);
    if (rec?.kind === "filed") location.hash = HASH.backlog;
    else if (rec) setOpened(rec.key);
  };
  // Esc puts nothing back in the pane: the composer, with the keys still the list's.
  const close = () => {
    setOpened(null);
    setSel(-1);
    if (wide && opened === null && location.hash !== "#/new") location.hash = "#/new";
  };
  // n is the composer beside the list, or the launcher without the room for one.
  const compose = () => {
    if (!wide) return void (location.hash = "#/new");
    // Already showing: just the keyboard. Otherwise it takes it once it's drawn.
    if (home) return focusComposer();
    focusComposer();
    location.hash = "#/new";
  };
  const to = (hash: string) => () => (location.hash = hash);
  // A phone at inbox zero pins the composer over the tab bar; with anything
  // waiting only the + shows (#275).
  const phoneHome = !wide && !desktop && needs.length === 0;
  // Beside an open session, the keys are that session's.
  usePageKeys(
    "inbox",
    {
      n: compose,
      b: to("#/backlog"),
      d: to(HASH.memory),
      a: to(HASH.history),
      h: to(HASH.history),
      s: to(HASH.supervisor),
      "/": () => (home ? focusComposer() : (location.hash = HASH.search)),
      ",": to("#/settings"),
      j: () => pick(sel + 1),
      k: () => pick(sel - 1),
      Enter: open,
      o: open,
      Escape: close,
      r: () => sessionOf(selected) !== null && (location.hash = `#/s/${sessionOf(selected)}/review`),
    },
    { off: beside != null },
  );

  const row = ({ s, n }: { s: SessionInfo; n: number }) => (
    <SessionRow
      key={s.id}
      s={s}
      n={n}
      picked={selected === `s${s.id}`}
      current={beside === s.id}
      swiped={swiped === s.id}
      onSwipe={(open) => setSwiped((was) => (open ? s.id : was === s.id ? null : was))}
    />
  );

  return (
    <div className="page">
      {hasRain(look) && <Rain key={look} />}
      <header className="topbar">
        <h1 className="sr-only">Sessions</h1>
        <div className="brand">
          <Mark size={26} />
          <Wordmark />
        </div>
        <ConnectionPill />
        <button
          className="icon-btn"
          aria-label="New session"
          onClick={() => (location.hash = "#/new")}
        >
          <Plus />
        </button>
        {/* A desktop has Settings at the foot of its rail; a phone's tab bar is the four sections. */}
        {!desktop && (
          <button className="icon-btn" aria-label="Settings" onClick={() => (location.hash = HASH.settings)}>
            <Gear />
          </button>
        )}
      </header>

      <main className={`inbox ${phoneHome ? "with-composer" : ""}`}>
        {wide && (
          <div className={`row-slot new-row-slot ${selected === "new" ? "picked" : ""}`} data-pick>
            <button
              className={`row new-row ${selected === "new" ? "picked" : ""} ${home ? "current" : ""}`}
              aria-current={home || undefined}
              onClick={() => (location.hash = "#/new")}
            >
              <Plus />
              <span className="row-main">
                <span className="row-name">New session</span>
              </span>
              <kbd className="new-row-key">n</kbd>
            </button>
          </div>
        )}
        <section>
          <h2 className="section-title">
            Needs you <span className="count">{needs.length}</span>
          </h2>
          <AllowAll />
          {needs.length === 0 ? (
            <div className="calm">
              <div className="calm-mark">✓</div>
              <div className="calm-title">All clear</div>
              <div className="calm-sub">
                {working
                  ? `${working} working · nothing needs you`
                  : "Nothing needs you"}
              </div>
              {sessions.length === 0 && (
                <div className="calm-tutor">
                  New here? <TutorButton label="Start the tutor" />
                </div>
              )}
            </div>
          ) : (
            <>
              {waiting.length > 0 && (
                <div className="cards">
                  {waiting.map((item) => (
                    <QueueCard
                      key={item.session}
                      item={item}
                      info={sessions.find((s) => s.id === item.session)}
                      picked={selected === `s${item.session}`}
                      current={beside === item.session}
                    />
                  ))}
                </div>
              )}
              {records.length > 0 && (
                <div className="list attn-records">
                  {records.map((a) => (
                    <RecordRow
                      key={a.key}
                      a={a}
                      picked={selected === a.key}
                      open={opened === a.key}
                      current={pane === a.key}
                      onToggle={() => setOpened((was) => (was === a.key ? null : a.key))}
                    />
                  ))}
                </div>
              )}
            </>
          )}
        </section>

        {phoneHome && <ReadyList compact />}

        <div className="quiet">
          <QuotaMeter quiet />
          <NotifyHint />
        </div>

        {running.length > 0 && (
          <section>
            <h2 className="section-title">
              Running <span className="count">{running.length}</span>
            </h2>
            <div className="list">{running.map(row)}</div>
          </section>
        )}

        {(quiet.length > 0 || sessions.length === 0) && (
          <section>
            <h2 className="section-title">
              Quiet <span className="count">{quiet.length}</span>
            </h2>
            <div className="list">
              {quiet.map(row)}
              {sessions.length === 0 && (
                <EmptyState
                  title="No sessions yet"
                  hint="Start an agent in a repository and it shows up here."
                  action={{ label: "New session", run: () => (location.hash = "#/new") }}
                />
              )}
            </div>
          </section>
        )}
      </main>
      {phoneHome && <Composer phone />}
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
      const about = weakSummary(q.status.summary)
        ? (prompts[q.session]?.context ?? q.status.summary)
        : q.status.summary;
      return { item: q, choices, yes: plainYes(choices), about };
    })
    .filter(
      (
        x,
      ): x is {
        item: QueueItem;
        choices: Choice[];
        yes: Choice;
        about: string | null;
      } => x.yes !== null,
    );
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
            void answerAll(
              chosen.map((x) => ({
                session: x.item.session,
                choices: x.choices,
                choice: x.yes,
              })),
            );
          }}
        >
          Allow {chosen.length}
        </button>
      </div>
    </div>
  );
}

function QueueCard({
  item,
  info,
  picked,
  current,
}: {
  item: QueueItem;
  info?: SessionInfo;
  picked: boolean;
  /** Open in the pane beside the list. */
  current: boolean;
}) {
  const now = useApp((s) => s.now);
  const prompt = useApp((s) => s.prompts[item.session]);
  const sent = useApp((s) => answeringLabel(s.answering[item.session], prompt?.choices ?? [], item.status.seq));
  const s = item.status;
  // A flagged PR is Stuck whatever the turn did: its dot says so.
  const state = sessionKind(item) === "stuck" && s.state !== "stale" && s.state !== "interrupted" ? "blocked" : s.state;
  const choices = prompt?.choices ?? [];
  return (
    <div
      className={`card-slot ${picked ? "picked" : ""}`}
      data-pick
      data-glide
      style={{ "--vtn": `card-${item.session}` } as CSSProperties}
    >
      <article className={`card state-${state} ${current ? "current" : ""}`}>
        <button className="card-head" aria-current={current || undefined} onClick={() => go(item.session)}>
          <StateIcon state={state} />
          <span className="card-name">{item.name}</span>
          <span className="card-prog">{info ? program(info) : s.agent}</span>
          <span className="card-age">{age(s.since_ms, now)}</span>
        </button>
        <div className="card-state">
          {STATE_LABEL[s.state]}
          {item.report ? <span className="report"> · {item.report}</span> : null}
        </div>
        {weakSummary(s.summary) && prompt?.context ? (
          <p className="card-summary">{prompt.context}</p>
        ) : (
          s.summary && <p className="card-summary">{s.summary}</p>
        )}
        {item.flag && <StepOffer item={item} />}
        {choices.length > 0 && (
          <div className={`choices ${sent !== null ? "answering" : ""}`} aria-busy={sent !== null}>
            {choices.map((c, i) => (
              <button
                key={i}
                className={`choice ${isNegative(c.label) ? "no" : i === 0 ? "yes" : ""} ${sent === c.label ? "sent" : ""}`}
                disabled={sent !== null}
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
            <button
              className="ghost"
              onClick={() => (location.hash = `#/s/${item.session}/review`)}
            >
              Review changes
            </button>
          )}
        </div>
      </article>
    </div>
  );
}

/**
 * The supervisor's offer on a flagged session (ADR-0019 §2): why it picked its
 * step, and one tap to take it, the other that would fit, or leave the flag.
 */
function StepOffer({ item }: { item: QueueItem }) {
  const [busy, setBusy] = useState(false);
  const flag = item.flag!;
  const choices = stepChoices(flag);
  if (choices.length === 0)
    return flag.working ? (
      <p className="step-why">Working out what to offer…</p>
    ) : null;
  return (
    <div className="step-offer">
      <p className="step-why">
        <span className="step-label">Supervisor:</span> {flag.step!.why}
      </p>
      <div className="step-actions">
        {choices.map((c) => (
          <button
            key={c.action}
            className={c.primary ? "primary" : "ghost"}
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              await takeStep(item.session, flag.seq, c.action);
              setBusy(false);
            }}
          >
            {c.label}
          </button>
        ))}
      </div>
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
  current,
  swiped,
  onSwipe,
}: {
  s: SessionInfo;
  n: number;
  picked: boolean;
  /** Open in the pane beside the list. */
  current: boolean;
  swiped: boolean;
  onSwipe: (open: boolean) => void;
}) {
  const now = useApp((st) => st.now);
  const queued = useApp((st) => st.queue.some((q) => q.session === s.id));
  const state = s.exited !== null ? "exited" : s.status.state;
  // Where the row is while a finger drags it; null when it rests.
  const [drag, setDrag] = useState<number | null>(null);
  // Null until the finger has moved far enough to tell a swipe from a scroll.
  const gesture = useRef<{
    x: number;
    y: number;
    sideways: boolean | null;
  } | null>(null);
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
    <div
      className="row-slot"
      data-pick
      data-glide
      style={{ "--vtn": `row-${s.id}` } as CSSProperties}
    >
      <button
        className="row-close"
        tabIndex={swiped ? 0 : -1}
        aria-hidden={!swiped}
        onClick={() => void kill(s.id)}
      >
        {state === "exited" ? "Remove" : "Close"}
      </button>
      <button
        aria-current={current || undefined}
        className={`row ${queued ? "queued" : ""} ${picked ? "picked" : ""} ${current ? "current" : ""} ${drag !== null ? "dragging" : ""}`}
        style={x ? { transform: `translateX(${x}px)` } : undefined}
        onPointerDown={(e) => {
          swallow.current = false;
          if (e.pointerType !== "mouse")
            gesture.current = { x: e.clientX, y: e.clientY, sideways: null };
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
          <span
            className={`row-name state-text-${queued ? state : "plain"}${s.auto_name ? " auto-name" : ""}`}
          >
            {s.name}
          </span>
          <span className="row-sub">
            {program(s)} ·{" "}
            {state === "exited"
              ? `exited ${s.exited ?? ""}`
              : STATE_LABEL[state]}
            {s.status.summary ? ` · ${s.status.summary}` : ""}
            {s.report ? <span className="report"> · {s.report}</span> : null}
          </span>
          {s.overlap ? (
            <span className="row-sub row-overlap">⚠ {s.overlap}</span>
          ) : null}
        </span>
        <span className="row-age">{age(s.status.since_ms, now)}</span>
        <Chevron />
      </button>
    </div>
  );
}
