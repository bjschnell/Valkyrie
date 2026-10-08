import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { STATE_LABEL, program } from "../proto";
import { isNegative } from "../lib/choices";
import type { Key } from "../lib/keys";
import { cssStyle, reflow, rowRuns, type Run } from "../lib/screen";
import { age } from "../lib/time";
import { answer, kill, openChoices, rename, sendKey, sendMessage, useApp } from "../store";
import { Back, Dots, Grid, Send, Wrap } from "./icons";
import { ConnectionPill, StateIcon } from "./bits";

const VIEW_KEY = "valk.view";
const DRAFT_KEY = (id: number) => `valk.draft.${id}`;

/** Reflowed on a phone (lines wrap at its width), the exact grid on a wide screen. */
function defaultView(): "reflow" | "grid" {
  const saved = localStorage.getItem(VIEW_KEY);
  if (saved === "reflow" || saved === "grid") return saved;
  return window.innerWidth < 760 ? "reflow" : "grid";
}

export function SessionView({ id }: { id: number }) {
  const info = useApp((s) => s.sessions.find((x) => x.id === id));
  const screen = useApp((s) => s.screen);
  const exited = useApp((s) => s.exited);
  const now = useApp((s) => s.now);
  const [view, setView] = useState(defaultView);
  const [menu, setMenu] = useState(false);
  const choices = useMemo(() => openChoices(screen), [screen]);
  const state = exited || info?.exited != null ? "exited" : (info?.status.state ?? "idle");

  const toggleView = () => {
    const next = view === "reflow" ? "grid" : "reflow";
    localStorage.setItem(VIEW_KEY, next);
    setView(next);
  };

  return (
    <div className="page session">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => history.length > 1 ? history.back() : (location.hash = "#/")}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">{info?.name ?? `#${id}`}</div>
          <div className="title-sub">
            <StateIcon state={state} />
            {info ? program(info) : ""} · {STATE_LABEL[state]}
            {info && <span className="muted"> · {age(info.status.since_ms, now)}</span>}
          </div>
        </div>
        <ConnectionPill />
        <button className="icon-btn" aria-label={view === "reflow" ? "Show the grid" : "Reflow"} onClick={toggleView}>
          {view === "reflow" ? <Grid /> : <Wrap />}
        </button>
        <button className="icon-btn" aria-label="Session menu" onClick={() => setMenu((m) => !m)}>
          <Dots />
        </button>
        {menu && <SessionMenu id={id} name={info?.name ?? ""} agent={info ? program(info) : ""} onClose={() => setMenu(false)} />}
      </header>

      <ScreenView view={view} />

      {exited && (
        <div className="banner">
          Exited{exited.code !== null ? ` with code ${exited.code}` : ""}.
        </div>
      )}

      {!exited && (
        <footer className="dock">
          {choices.length > 0 && (
            <div className="choices dock-choices">
              {choices.map((c, i) => (
                <button
                  key={i}
                  className={`choice ${isNegative(c.label) ? "no" : i === 0 ? "yes" : ""} ${c.selected ? "selected" : ""}`}
                  onClick={() => void answer(id, choices, c)}
                >
                  {c.n !== undefined && <span className="choice-n">{c.n}</span>}
                  {c.label}
                </button>
              ))}
            </div>
          )}
          <KeyBar />
          <Composer id={id} />
        </footer>
      )}
    </div>
  );
}

function ScreenView({ view }: { view: "reflow" | "grid" }) {
  const screen = useApp((s) => s.screen);
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // Follow new output unless you scrolled up to read.
  const onScroll = () => {
    const el = ref.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [screen, view]);

  if (!screen) return <div className="screen loading">Opening…</div>;
  const lines: Run[][] = view === "reflow" ? reflow(screen) : screen.rows.map(rowRuns);
  return (
    <div ref={ref} className={`screen ${view}`} onScroll={onScroll}>
      <pre style={view === "grid" ? { width: `${screen.cols}ch` } : undefined}>
        {lines.map((runs, i) => (
          <div className="line" key={i}>
            {runs.length === 0
              ? " "
              : runs.map((r, j) => (
                  <span key={j} style={cssStyle(r.style)}>
                    {r.text}
                  </span>
                ))}
          </div>
        ))}
      </pre>
    </div>
  );
}

const KEYS: [Key, string][] = [
  ["esc", "esc"],
  ["tab", "⇥"],
  ["shift_tab", "⇧⇥"],
  ["up", "↑"],
  ["down", "↓"],
  ["left", "←"],
  ["right", "→"],
  ["ctrl_c", "^C"],
  ["enter", "⏎"],
];

/** Keys a phone keyboard lacks. Esc stops an agent's turn; ⇧⇥ cycles Claude's modes. */
function KeyBar() {
  return (
    <div className="keybar">
      {KEYS.map(([key, label]) => (
        <button
          key={key}
          className={`key key-${key}`}
          // Keep the phone keyboard up while tapping keys.
          onPointerDown={(e) => e.preventDefault()}
          onClick={() => {
            navigator.vibrate?.(5);
            sendKey(key);
          }}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

function Composer({ id }: { id: number }) {
  const [text, setText] = useState(() => localStorage.getItem(DRAFT_KEY(id)) ?? "");
  const area = useRef<HTMLTextAreaElement>(null);

  // A draft survives the app being closed mid-sentence.
  useEffect(() => {
    if (text) localStorage.setItem(DRAFT_KEY(id), text);
    else localStorage.removeItem(DRAFT_KEY(id));
  }, [id, text]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [text]);

  const send = () => {
    if (sendMessage(text)) setText("");
  };
  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        send();
      }}
    >
      <textarea
        ref={area}
        rows={1}
        value={text}
        placeholder="Message the session"
        enterKeyHint="send"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // A desktop keyboard sends with Enter (Shift-Enter for a new line);
          // a phone's return key adds a line, its send button sends.
          if (e.key === "Enter" && !e.shiftKey && window.matchMedia("(pointer: fine)").matches) {
            e.preventDefault();
            send();
          }
        }}
      />
      <button className="send" aria-label="Send" disabled={!text.trim()}>
        <Send />
      </button>
    </form>
  );
}

function SessionMenu({ id, name, agent, onClose }: { id: number; name: string; agent: string; onClose: () => void }) {
  const [confirm, setConfirm] = useState(false);
  return (
    <>
      <div className="scrim" onClick={onClose} />
      <div className="menu" role="menu">
        <button
          role="menuitem"
          onClick={() => {
            onClose();
            const next = prompt("Name this session (blank: its folder)", name);
            if (next !== null) void rename(id, next.trim() || null);
          }}
        >
          Rename
        </button>
        <button
          role="menuitem"
          className="danger"
          onClick={() => {
            if (!confirm) return setConfirm(true);
            onClose();
            void kill(id);
            location.hash = "#/";
          }}
        >
          {confirm ? `Close ${agent}? Tap again` : "Close"}
        </button>
      </div>
    </>
  );
}
