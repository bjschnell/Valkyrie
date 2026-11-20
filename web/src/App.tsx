import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { Backlog, ItemEditor, OrderPage } from "./components/Backlog";
import { DecisionList } from "./components/DecisionList";
import { DigestView } from "./components/Digest";
import { PastView } from "./components/Past";
import { WikiView } from "./components/Wiki";
import { SearchView } from "./components/Search";
import { SupervisorView } from "./components/Supervisor";
import { DocView, DocsList } from "./components/Docs";
import { Inbox } from "./components/Inbox";
import { Launcher } from "./components/Launcher";
import { Outdated } from "./components/Outdated";
import { Pair } from "./components/Pair";
import { ReviewView } from "./components/Review";
import { SessionView } from "./components/SessionView";
import { MAC, useDesktop, useWide } from "./components/Terminal";
import { chord, chordTarget } from "./lib/chords";
import { showNeeds } from "./lib/badge";
import { isRecord, RecordCard, useAttention } from "./components/Attention";
import { waitingSessions } from "./lib/attention";
import { follow, typingIn } from "./lib/follow";
import { HASH, moved, SECTION_KEYS } from "./lib/routes";
import { paletteKey } from "./lib/palette";
import { shortcutsKey } from "./lib/shortcuts";
import { Palette } from "./components/Palette";
import { Shortcuts } from "./components/Shortcuts";
import { Settings } from "./components/Settings";
import { OfflineBanner } from "./components/states";
import { Toast } from "./components/Toast";
import { Rail, TabBar, type NavKey } from "./components/Nav";
import { close, connect, open, useApp } from "./store";

/** Alt on most keyboards, Option on a Mac's. */
const ALT = MAC ? "Option" : "Alt";

type Route =
  | { page: "inbox" }
  | { page: "new" }
  | { page: "settings" }
  | { page: "backlog" }
  | { page: "decisions" }
  | { page: "docs" }
  | { page: "digest" }
  | { page: "past" }
  | { page: "wiki" }
  | { page: "search" }
  | { page: "supervisor" }
  | { page: "doc"; key: string }
  | { page: "item"; id: number | null }
  | { page: "order"; project: string }
  | { page: "session"; id: number }
  | { page: "review"; id: number };

/**
 * `#/s/12` is session 12, `#/s/12/review` its changes, `#/new` the launcher,
 * `#/settings` settings, `#/backlog` the backlog, `#/backlog/new` and `#/backlog/3`
 * writing an item down and changing item 3, `#/backlog/order/<project>` a repository's build order.
 * Memory (`#/memory`: decisions, skills, policies, habits, notes) has the tabs `#/memory/docs`
 * (and `#/memory/docs/12`, one doc by its key, never a path) and `#/memory/wiki`. History
 * (`#/history`: past sessions to resume) has `#/history/summary`, `#/history/search` and
 * `#/history/supervisor`. The pages those used to be (`#/docs`, `#/summary`…) redirect (lib/routes.ts).
 */
function route(): Route {
  const to = moved(location.hash);
  // In place, so Back doesn't land on the old address and bounce.
  if (to) history.replaceState(history.state, "", to);
  const hash = location.hash;
  const m = /^#\/s\/(\d+)(\/review)?$/.exec(hash);
  if (m) return { page: m[2] ? "review" : "session", id: Number(m[1]) };
  if (hash === "#/new") return { page: "new" };
  if (hash === HASH.settings) return { page: "settings" };
  if (hash === HASH.backlog) return { page: "backlog" };
  if (hash === HASH.memory) return { page: "decisions" };
  if (hash === HASH.docs) return { page: "docs" };
  if (hash === HASH.wiki) return { page: "wiki" };
  if (hash === HASH.history) return { page: "past" };
  if (hash === HASH.summary) return { page: "digest" };
  if (hash === HASH.search) return { page: "search" };
  if (hash === HASH.supervisor) return { page: "supervisor" };
  const d = /^#\/memory\/docs\/([^/]+)$/.exec(hash);
  // A key is digits or `p` and hex, so a hand-typed `%` is just not a doc.
  if (d) return { page: "doc", key: d[1] };
  const o = /^#\/backlog\/order\/(.+)$/.exec(hash);
  if (o) {
    try {
      return { page: "order", project: decodeURIComponent(o[1]) };
    } catch {
      return { page: "backlog" };
    }
  }
  const b = /^#\/backlog\/(new|\d+)$/.exec(hash);
  if (b) return { page: "item", id: b[1] === "new" ? null : Number(b[1]) };
  return { page: "inbox" };
}

/** Which of the four sections a page is in. */
function navKey(r: Route): NavKey | null {
  switch (r.page) {
    case "inbox":
    case "session":
    case "review":
      return "inbox";
    case "backlog":
    case "item":
    case "order":
      return "backlog";
    case "decisions":
    case "docs":
    case "doc":
    case "wiki":
      return "memory";
    case "past":
    case "digest":
    case "search":
    case "supervisor":
      return "history";
    case "settings":
      return "settings";
    case "new":
      return null;
  }
}

/** Pages reached from the nav, where a phone shows its tab bar. Deeper pages
 * (a session, a doc, an item) keep their back arrow and the whole screen. */
const TOP = new Set<Route["page"]>(["inbox", "backlog", "docs", "decisions", "digest", "past", "wiki", "search", "supervisor", "settings"]);

/** How deep a page is, so a transition knows forward from back. */
const depth = (r: Route) => (TOP.has(r.page) ? 0 : r.page === "review" ? 2 : 1);

export function App() {
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [at, setAt] = useState(route);
  const atRef = useRef(at);
  atRef.current = at;

  // The session the right pane opened by itself (#248), while it's followed.
  const followed = useRef<{ pending: number | null; id: number | null }>({ pending: null, id: null });

  useEffect(() => {
    const onHash = () => {
      const next = route();
      const from = atRef.current;
      // Any other move (a click, a chord, Back) is yours, and ends the following.
      const f = followed.current;
      f.id = next.page === "session" && next.id === f.pending ? f.pending : null;
      f.pending = null;
      const swap = () => flushSync(() => setAt(next));
      // A transition between pages. Stepping between sessions
      // (Alt+J/K) stays instant; so does a device asking for less motion.
      if (
        !document.startViewTransition ||
        matchMedia("(prefers-reduced-motion: reduce)").matches ||
        (from.page === "session" && next.page === "session")
      ) {
        setAt(next);
        return;
      }
      const d = depth(next) - depth(from);
      document.documentElement.dataset.vt = d > 0 ? "forward" : d < 0 ? "back" : "fade";
      document.startViewTransition(swap);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  // A new page takes the keyboard to its heading, so a screen reader says where
  // you are and Tab starts from the top of it (#188). Not on the first load,
  // and not when the page already put you in a field (search, a new item).
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (document.activeElement?.closest("input, textarea, select, [contenteditable]")) return;
    // Beside the session list, the open pane's heading is the new page's.
    const heading = document.querySelector<HTMLElement>(".split-pane h1") ?? document.querySelector<HTMLElement>("h1");
    if (!heading) return;
    heading.tabIndex = -1;
    heading.focus({ preventScroll: true });
  }, [at]);

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
  // Who needs you, on the app icon and the tab title (#187): the attention
  // queue's one number, sessions and records alike (#251).
  const attn = useAttention();
  const needs = attn.length;
  // The waiting sessions in the queue's order, for Alt+N's key handler.
  const waitingNow = useRef<number[]>([]);
  waitingNow.current = waitingSessions(attn);
  useEffect(() => showNeeds(paired ? needs : 0), [paired, needs]);
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
      const { sessions } = useApp.getState();
      const to = chordTarget(pressed, {
        sessions: sessions.map((s) => s.id),
        needs: waitingNow.current,
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

  // The command palette (#183): Ctrl+K, or Cmd+K on a Mac, from anywhere on a
  // desktop keyboard. A session's terminal keeps Ctrl+K (kill-line) while it
  // has the keyboard, so Ctrl+Shift+K opens it from there.
  const [palette, setPalette] = useState(false);
  const closePalette = useCallback(() => setPalette(false), []);
  useEffect(() => {
    if (!desktop || !paired) return;
    const onKey = (e: KeyboardEvent) => {
      const terminal = !!document.querySelector(".typing-field") && !(e.target as Element | null)?.closest?.(".palette");
      const pressed = {
        key: e.key,
        code: e.code,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        metaKey: e.metaKey,
        altGraph: e.getModifierState("AltGraph"),
      };
      if (e.repeat || !paletteKey(pressed, MAC, terminal)) return;
      // The browser's own Ctrl+K is its search bar.
      e.preventDefault();
      setPalette((p) => !p);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [desktop, paired]);
  useEffect(() => {
    if (!desktop || !paired) setPalette(false);
  }, [desktop, paired]);

  // g then i, b, m or h goes to a section (#249), when you aren't typing. Not on
  // a session or its review, where g is the page's own (top of the screen).
  const page = at.page;
  useEffect(() => {
    if (!desktop || !paired || page === "session" || page === "review") return;
    let armed = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.repeat) return;
      if ((e.target as Element | null)?.closest?.("textarea, input, select, [contenteditable], dialog")) return;
      const to = SECTION_KEYS[e.key];
      if (armed && Date.now() - armed < 1500 && to) {
        armed = 0;
        e.preventDefault();
        // The second key is the chord's, not the page's (Inbox has its own b and h).
        e.stopImmediatePropagation();
        if (location.hash !== to) location.hash = to;
        return;
      }
      armed = e.key === "g" ? Date.now() : 0;
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [desktop, paired, page]);

  // The '?' sheet (#189): the keys of the page you're on, from anywhere you
  // aren't typing (a terminal that has the keyboard is typing: the palette's
  // "Keyboard shortcuts" opens it there).
  const [sheet, setSheet] = useState(false);
  const closeSheet = useCallback(() => setSheet(false), []);
  const openSheet = useCallback(() => {
    // After the palette has handed the keyboard back, so the sheet gives it back there.
    setTimeout(() => setSheet(true));
  }, []);
  useEffect(() => {
    if (!desktop || !paired) return;
    const onKey = (e: KeyboardEvent) => {
      const pressed = {
        key: e.key,
        code: e.code,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        metaKey: e.metaKey,
        altGraph: e.getModifierState("AltGraph"),
      };
      if (e.repeat || !shortcutsKey(pressed, e.target as Element | null)) return;
      e.preventDefault();
      setSheet((s) => !s);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [desktop, paired]);
  useEffect(() => {
    if (!desktop || !paired) setSheet(false);
  }, [desktop, paired]);

  // With the room for it, the session list stays beside the open session.
  const wide = useWide();

  // Beside the list, the pane shows the top waiting session rather than
  // nothing, and moves on to the next once it's answered (#248). With only
  // records waiting, it shows the top one's card (#251).
  const waiting = waitingSessions(attn).join(",");
  const topRecord = attn.find(isRecord);
  useEffect(() => {
    if (!wide || !paired || (at.page !== "inbox" && at.page !== "session")) return;
    const open = at.page === "session" ? at.id : null;
    const queue = waiting ? waiting.split(",").map(Number) : [];
    const to = follow({
      open,
      followed: followed.current.id,
      queue,
      typing: open !== null && typingIn(open, document.querySelector(".split-pane")),
    });
    if (!to || to.open === open) return;
    followed.current.pending = to.open;
    location.replace(`#/s/${to.open}`);
  }, [wide, paired, at, waiting]);

  // A rail on a desktop, a tab bar on a phone's top pages.
  const rail = desktop && paired;
  const tabs = !desktop && paired && TOP.has(at.page);
  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("with-rail", rail);
    root.classList.toggle("with-tabs", tabs);
    root.dataset.page = at.page;
  }, [rail, tabs, at.page]);
  const chrome = (
    <>
      {rail && <Rail at={navKey(at)} />}
      {tabs && <TabBar at={navKey(at)} />}
      {palette && <Palette onClose={closePalette} onShortcuts={openSheet} />}
      {sheet && <Shortcuts page={at.page} onClose={closeSheet} />}
      <OfflineBanner />
      <Toast />
    </>
  );

  if (!token || status === "unpaired") return <Pair />;
  if (status === "outdated") return <Outdated />;
  if (wide && (at.page === "inbox" || at.page === "session" || at.page === "review")) {
    return (
      <>
        <div className="split">
          <Inbox beside={openId} pane={at.page === "inbox" && !waiting ? topRecord?.key : null} />
          <div className="split-pane">
            {at.page === "session" && <SessionView id={at.id} />}
            {at.page === "review" && <ReviewView key={at.id} id={at.id} />}
            {at.page === "inbox" && !waiting && topRecord && (
              <div className="split-record">
                <RecordCard key={topRecord.key} a={topRecord} />
              </div>
            )}
            {at.page === "inbox" && !waiting && !topRecord && (
              <div className="split-empty">
                <div className="split-empty-title">Nothing needs you</div>
                <div>
                  Pick a session from the list. <kbd>{ALT}</kbd>+<kbd>J</kbd> and <kbd>{ALT}</kbd>+<kbd>K</kbd> step through
                  them, <kbd>{ALT}</kbd>+<kbd>N</kbd> goes to whoever needs you, <kbd>{MAC ? "⌘" : "Ctrl"}</kbd>+<kbd>K</kbd> to anything.
                </div>
              </div>
            )}
          </div>
        </div>
        {chrome}
      </>
    );
  }
  // The item editor is a panel beside the backlog's list when there's room (#252).
  if (wide && at.page === "item") {
    return (
      <>
        <div className="split split-side">
          <Backlog beside />
          <div className="split-pane">
            <ItemEditor key={at.id ?? "new"} id={at.id} />
          </div>
        </div>
        {chrome}
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
      {at.page === "docs" && <DocsList />}
      {at.page === "digest" && <DigestView />}
      {at.page === "past" && <PastView />}
      {at.page === "wiki" && <WikiView />}
      {at.page === "search" && <SearchView />}
      {at.page === "supervisor" && <SupervisorView />}
      {at.page === "doc" && <DocView key={at.key} docKey={at.key} />}
      {at.page === "item" && <ItemEditor key={at.id ?? "new"} id={at.id} />}
      {at.page === "order" && <OrderPage key={at.project} project={at.project} />}
      {at.page === "session" && <SessionView id={at.id} />}
      {at.page === "review" && <ReviewView id={at.id} />}
      {chrome}
    </>
  );
}
