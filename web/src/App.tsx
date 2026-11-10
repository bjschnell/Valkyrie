import { useEffect, useRef, useState } from "react";
import { Backlog, ItemEditor } from "./components/Backlog";
import { DecisionList } from "./components/DecisionList";
import { Inbox } from "./components/Inbox";
import { Launcher } from "./components/Launcher";
import { Outdated } from "./components/Outdated";
import { Pair } from "./components/Pair";
import { ReviewView } from "./components/Review";
import { SessionView } from "./components/SessionView";
import { MAC, useDesktop, useWide } from "./components/Terminal";
import { chord, chordTarget } from "./lib/chords";
import { Settings } from "./components/Settings";
import { Toast } from "./components/Toast";
import { close, connect, open, useApp } from "./store";

/** Alt on most keyboards, Option on a Mac's. */
const ALT = MAC ? "Option" : "Alt";

type Route =
  | { page: "inbox" }
  | { page: "new" }
  | { page: "settings" }
  | { page: "backlog" }
  | { page: "decisions" }
  | { page: "item"; id: number | null }
  | { page: "session"; id: number }
  | { page: "review"; id: number };

/**
 * `#/s/12` is session 12, `#/s/12/review` its changes, `#/new` the launcher,
 * `#/settings` settings, `#/backlog` the backlog, `#/backlog/new` and `#/backlog/3`
 * writing an item down and changing item 3, `#/decisions` every active decision.
 */
function route(): Route {
  const m = /^#\/s\/(\d+)(\/review)?$/.exec(location.hash);
  if (m) return { page: m[2] ? "review" : "session", id: Number(m[1]) };
  if (location.hash === "#/new") return { page: "new" };
  if (location.hash === "#/settings") return { page: "settings" };
  if (location.hash === "#/backlog") return { page: "backlog" };
  if (location.hash === "#/decisions") return { page: "decisions" };
  const b = /^#\/backlog\/(new|\d+)$/.exec(location.hash);
  if (b) return { page: "item", id: b[1] === "new" ? null : Number(b[1]) };
  return { page: "inbox" };
}

export function App() {
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [at, setAt] = useState(route);

  useEffect(() => {
    const onHash = () => setAt(route());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (token) connect();
  }, [token]);

  // A session's page and its review both keep it open (its screen has the prompt).
  const openId = at.page === "session" || at.page === "review" ? at.id : null;
  useEffect(() => {
    if (openId === null) void close();
    else void open(openId);
  }, [openId]);

  // On a desktop keyboard the TUI's Alt chords move between sessions, from
  // anywhere: the terminal leaves them alone even while it has the keyboard.
  const desktop = useDesktop();
  // The sessions you've been on, latest last, for Alt-P.
  const seen = useRef<{ open: number | null; recent: number[] }>({ open: null, recent: [] });
  if (seen.current.open !== openId) {
    const recent = seen.current.recent.filter((id) => id !== openId);
    seen.current = { open: openId, recent: openId === null ? recent : [...recent, openId] };
  }
  const paired = !!token && status !== "unpaired" && status !== "outdated";
  useEffect(() => {
    if (!desktop || !paired) return;
    const onKey = (e: KeyboardEvent) => {
      const pressed = chord(
        {
          key: e.key,
          code: e.code,
          ctrlKey: e.ctrlKey,
          altKey: e.altKey,
          shiftKey: e.shiftKey,
          metaKey: e.metaKey,
          altGraph: e.getModifierState("AltGraph"),
        },
        MAC,
      );
      if (!pressed) return;
      // In a text field a Mac's Option types (£, ∆, π): those are the field's.
      if (MAC && (e.target as HTMLElement).closest?.("textarea:not(.typing-field), input, [contenteditable]")) return;
      // The browser has uses for some of these (Alt+1 is its first tab on Linux).
      e.preventDefault();
      const { sessions, queue } = useApp.getState();
      const to = chordTarget(pressed, {
        sessions: sessions.map((s) => s.id),
        needs: queue.map((q) => q.session),
        open: seen.current.open,
        recent: seen.current.recent,
      });
      if (to === "home") location.hash = "#/";
      // Stepping through sessions is one place in the browser's history, not a trail.
      else if (to && seen.current.open !== null) location.replace(`#/s/${to.session}`);
      else if (to) location.hash = `#/s/${to.session}`;
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [desktop, paired]);

  // With the room for it, the session list stays beside the open session.
  const wide = useWide();

  if (!token || status === "unpaired") return <Pair />;
  if (status === "outdated") return <Outdated />;
  if (wide && (at.page === "inbox" || at.page === "session" || at.page === "review")) {
    return (
      <>
        <div className="split">
          <Inbox beside={openId} />
          <div className="split-pane">
            {at.page === "session" && <SessionView id={at.id} />}
            {at.page === "review" && <ReviewView key={at.id} id={at.id} />}
            {at.page === "inbox" && (
              <div className="split-empty">
                <div className="split-empty-title">No session open</div>
                <div>
                  Pick one from the list. <kbd>{ALT}</kbd>+<kbd>J</kbd> and <kbd>{ALT}</kbd>+<kbd>K</kbd> step through
                  them, <kbd>{ALT}</kbd>+<kbd>N</kbd> goes to whoever needs you.
                </div>
              </div>
            )}
          </div>
        </div>
        <Toast />
      </>
    );
  }
  return (
    <>
      {at.page === "inbox" && <Inbox />}
      {at.page === "new" && <Launcher />}
      {at.page === "settings" && <Settings />}
      {at.page === "backlog" && <Backlog />}
      {at.page === "decisions" && <DecisionList />}
      {at.page === "item" && <ItemEditor key={at.id ?? "new"} id={at.id} />}
      {at.page === "session" && <SessionView id={at.id} />}
      {at.page === "review" && <ReviewView id={at.id} />}
      <Toast />
    </>
  );
}
