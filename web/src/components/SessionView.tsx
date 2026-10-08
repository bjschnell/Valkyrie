import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { STATE_LABEL, program } from "../proto";
import { isNegative } from "../lib/choices";
import type { Key } from "../lib/keys";
import { cssStyle, reflow, rowRuns, type Run } from "../lib/screen";
import { age } from "../lib/time";
import { answer, kill, openChoices, rename, sendKey, sendMessage, useApp } from "../store";
import { Back, Dots, Grid, Send, Wrap } from "./icons";
import { ConnectionPill, StateIcon } from "./bits";
import { ChatView } from "./Chat";
import { MicButton } from "./Mic";

const VIEW_KEY = "valk.view";
const MODE_KEY = "valk.mode";
const DRAFT_KEY = (id: number) => `valk.draft.${id}`;

type Mode = "chat" | "terminal";

/** Chat when the session has an agent conversation, unless you chose the terminal. */
function savedMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "terminal" ? "terminal" : "chat";
  } catch {
    return "chat";
  }
}

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
  const [chosen, setChosen] = useState(savedMode);
  const [menu, setMenu] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const composer = useRef<HTMLTextAreaElement>(null);
  const choices = useMemo(() => openChoices(screen), [screen]);
  const state = exited || info?.exited != null ? "exited" : (info?.status.state ?? "idle");
  const hasChat = !!info?.chat;
  const mode: Mode = hasChat ? chosen : "terminal";

  const choose = (next: Mode) => {
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      /* remembered for this visit only */
    }
    setChosen(next);
  };

  // Keys on a desktop keyboard, vim-style, when you aren't typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("textarea, input, [contenteditable]") || e.altKey || e.metaKey) return;
      const el = scroller.current;
      const page = (el?.clientHeight ?? 400) / 2;
      const by = (dy: number) => el?.scrollBy({ top: dy, behavior: "smooth" });
      const ctrl = e.ctrlKey;
      const actions: Record<string, () => void> = ctrl
        ? { d: () => by(page), u: () => by(-page) }
        : {
            j: () => by(80),
            k: () => by(-80),
            g: () => el?.scrollTo({ top: 0, behavior: "smooth" }),
            G: () => el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" }),
            i: () => composer.current?.focus(),
            t: () => hasChat && choose(mode === "chat" ? "terminal" : "chat"),
            r: () => (location.hash = `#/s/${id}/review`),
            q: () => (location.hash = "#/"),
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
        {mode === "terminal" && (
          <button className="icon-btn" aria-label={view === "reflow" ? "Show the grid" : "Reflow"} onClick={toggleView}>
            {view === "reflow" ? <Grid /> : <Wrap />}
          </button>
        )}
        <button className="icon-btn" aria-label="Session menu" onClick={() => setMenu((m) => !m)}>
          <Dots />
        </button>
        {menu && <SessionMenu id={id} name={info?.name ?? ""} agent={info ? program(info) : ""} onClose={() => setMenu(false)} />}
      </header>

      {hasChat && (
        <div className="modes" role="tablist">
          {(["chat", "terminal"] as const).map((m) => (
            <button key={m} role="tab" aria-selected={mode === m} className={mode === m ? "on" : ""} onClick={() => choose(m)}>
              {m === "chat" ? "Chat" : "Terminal"}
            </button>
          ))}
        </div>
      )}

      {mode === "chat" ? (
        <ChatView info={info} scroller={scroller} onTerminal={() => choose("terminal")} />
      ) : (
        <ScreenView view={view} scroller={scroller} />
      )}

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
          <Composer id={id} area={composer} />
        </footer>
      )}
    </div>
  );
}

function ScreenView({ view, scroller: ref }: { view: "reflow" | "grid"; scroller: RefObject<HTMLDivElement | null> }) {
  const screen = useApp((s) => s.screen);
  const stick = useRef(true);

  // Follow new output unless you scrolled up to read.
  const onScroll = () => {
    const el = ref.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [screen, view, ref]);

  if (!screen) return <div ref={ref} className="screen loading">Opening…</div>;
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

function Composer({ id, area }: { id: number; area: RefObject<HTMLTextAreaElement | null> }) {
  const [text, setText] = useState(() => localStorage.getItem(DRAFT_KEY(id)) ?? "");

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
  }, [text, area]);

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
          // Back to the keys above; the draft stays.
          if (e.key === "Escape") area.current?.blur();
        }}
      />
      <MicButton value={text} onChange={setText} />
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
          onClick={() => {
            onClose();
            location.hash = `#/s/${id}/review`;
          }}
        >
          Review changes
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
