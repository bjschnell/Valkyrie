// The command palette (#183): type to jump anywhere or run an action. Its
// entries and their order are lib/palette.ts; this is the box.

import { useEffect, useMemo, useRef, useState } from "react";
import { useCached } from "../cache";
import { entries, rank, type Entry } from "../lib/palette";
import { LOOK_TEXT, LOOKS, setLook, useLook } from "../lib/theme";
import { allDocs, changeItem, review, useApp } from "../store";
import { PAGES } from "./Nav";

/** Drawn at once; typing narrows the rest in. */
const SHOWN = 80;

const LOOK_LIST = LOOKS.map((key) => ({ key, ...LOOK_TEXT[key] }));

/** What each row is, said on its right. */
const GROUP: Record<Entry["group"], string> = {
  Actions: "Action",
  Sessions: "Session",
  Pages: "Page",
  Backlog: "Item",
  Docs: "Doc",
  Themes: "Theme",
};

function run(d: Entry["do"]) {
  if ("go" in d) {
    if (location.hash !== d.go) location.hash = d.go;
  } else if ("look" in d) setLook(d.look as (typeof LOOKS)[number]);
  else if ("decision" in d) void review(d.decision, { a: "accept" });
  else void changeItem(d.item, { do: "status", status: "open" });
}

export function Palette({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const sessions = useApp((s) => s.sessions);
  const queue = useApp((s) => s.queue);
  const proposals = useApp((s) => s.proposals);
  const items = useApp((s) => s.items);
  const orders = useApp((s) => s.orders);
  // What the Docs page last loaded, at once, then the daemon's (it marks none read).
  const docs = useCached("docs", allDocs).data?.docs;
  const look = useLook();
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);

  const all = useMemo(
    () =>
      entries({
        sessions,
        needs: queue.map((q) => q.session),
        proposals,
        items,
        orders,
        docs: docs ?? [],
        pages: PAGES,
        looks: LOOK_LIST,
        look,
      }),
    [sessions, queue, proposals, items, orders, docs, look],
  );
  const shown = useMemo(() => rank(all, query).slice(0, SHOWN), [all, query]);
  const pick = Math.min(at, Math.max(0, shown.length - 1));

  // Opened as a modal, so the rest of the page is inert under it; closed, the
  // keyboard goes back where it was (a session's terminal, say).
  useEffect(() => {
    const back = document.activeElement as HTMLElement | null;
    dialog.current?.showModal();
    return () => back?.focus?.({ preventScroll: true });
  }, []);
  // Moving away by any route (an Alt chord, the back button) closes it.
  useEffect(() => {
    window.addEventListener("hashchange", onClose);
    return () => window.removeEventListener("hashchange", onClose);
  }, [onClose]);
  useEffect(() => {
    list.current?.querySelector(`[data-i="${pick}"]`)?.scrollIntoView({ block: "nearest" });
  }, [pick]);

  const choose = (e: Entry | undefined) => {
    if (!e) return;
    onClose();
    run(e.do);
  };
  const move = (by: number) => shown.length && setAt((pick + by + shown.length) % shown.length);
  const onKey = (e: React.KeyboardEvent) => {
    const ctrl = e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey;
    if (e.key === "ArrowDown" || (ctrl && e.key === "n")) move(1);
    else if (e.key === "ArrowUp" || (ctrl && e.key === "p")) move(-1);
    else if (e.key === "Enter" && !e.nativeEvent.isComposing) choose(shown[pick]);
    else return;
    e.preventDefault();
  };

  return (
    <dialog
      ref={dialog}
      className="palette"
      aria-label="Command palette"
      onClose={onClose}
      onClick={(e) => e.target === dialog.current && onClose()}
    >
      <div className="palette-box">
        <input
          className="palette-input"
          autoFocus
          role="combobox"
          aria-expanded
          aria-controls="palette-list"
          aria-activedescendant={shown.length ? `palette-${pick}` : undefined}
          aria-autocomplete="list"
          placeholder="Jump to a session, page or doc, or run an action"
          spellCheck={false}
          autoComplete="off"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setAt(0);
          }}
          onKeyDown={onKey}
        />
        <div className="palette-list" id="palette-list" role="listbox" ref={list}>
          {shown.map((e, i) => (
            <div
              key={e.key}
              id={`palette-${i}`}
              data-i={i}
              role="option"
              aria-selected={i === pick}
              className="palette-row"
              onPointerMove={() => i !== pick && setAt(i)}
              onClick={() => choose(e)}
            >
              <span className="palette-main">
                <span className="palette-label">{e.label}</span>
                {e.sub && <span className="palette-sub">{e.sub}</span>}
              </span>
              <span className="palette-tag">{GROUP[e.group]}</span>
            </div>
          ))}
          {!shown.length && <div className="palette-empty">Nothing matches “{query}”</div>}
        </div>
        <div className="palette-foot" aria-hidden>
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>Enter</kbd> choose
          </span>
          <span>
            <kbd>Esc</kbd> close
          </span>
        </div>
      </div>
    </dialog>
  );
}
