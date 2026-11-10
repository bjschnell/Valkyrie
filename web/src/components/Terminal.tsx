// A session's screen: the grid or its reflowed lines, and at desktop width a
// terminal you type into (ADR-0020 phase 1). Click it and the keyboard is the
// session's: every key but the browser's own goes to the program as bytes, the
// session is sized to the pane, and the wheel pages back through its history.

import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
} from "react";
import { program, type Cursor, type Modes, type Row, type ScrollAnchor } from "../proto";
import { pathBytes } from "../lib/keys";
import { cellAt, mouseBytes, wantsMouse, type MouseAction } from "../lib/mouse";
import { cssStyle, reflow, rowRuns, textWidth, type Run } from "../lib/screen";
import { chord as shellChord } from "../lib/chords";
import { copiedRows, copiedText, type CopiedRow, encodeKey, fitSize, focusBytes, pasteBytes, wheelBytes } from "../lib/termkeys";
import { openModes, resizeOpen, scrollback, sendBytes, sendBytesTo, sendFile, toast, useApp } from "../store";

const DESKTOP = "(min-width: 760px) and (pointer: fine)";

/** Room for the session list beside a grid of that width. */
const WIDE = "(min-width: 1120px) and (pointer: fine)";

function useMedia(media: string): boolean {
  return useSyncExternalStore(
    (changed) => {
      const query = window.matchMedia(media);
      query.addEventListener("change", changed);
      return () => query.removeEventListener("change", changed);
    },
    () => window.matchMedia(media).matches,
  );
}

/** A screen wide enough for the grid, with a keyboard and a mouse. */
export const useDesktop = () => useMedia(DESKTOP);

/** A desktop screen wide enough for the session list beside the open session. */
export const useWide = () => useMedia(WIDE);

/** A page of history on show instead of the live screen. */
interface Back {
  fromTop: number;
  history: number;
  rows: Row[];
}

export const MAC = /Mac|iPhone|iPad/.test(navigator.platform);
/** Agents whose message box takes Esc Enter as a new line. */
const AGENTS = ["claude", "codex"];

const PROBE = 50;
/** A page of history that reaches into the live screen is fetched again this often. */
const REFRESH_MS = 200;
const RESIZE_MS = 120;

export function ScreenView({
  view,
  scroller: ref,
  typing,
  onTyping,
  live,
}: {
  view: "reflow" | "grid";
  scroller: RefObject<HTMLDivElement | null>;
  /** The keyboard is the session's. */
  typing: boolean;
  onTyping: (typing: boolean) => void;
  /** The session still runs, so it can be typed into. */
  live: boolean;
}) {
  const screen = useApp((s) => s.screen);
  const openId = useApp((s) => s.openId);
  const agent = useApp((s) => {
    const info = s.sessions.find((x) => x.id === s.openId);
    return !!info && AGENTS.includes(program(info));
  });
  const desktop = useDesktop();
  const [back, setBack] = useState<Back | null>(null);
  const stick = useRef(true);
  const pre = useRef<HTMLPreElement>(null);
  const probe = useRef<HTMLSpanElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  // Where the history on show should start, ahead of the reply that shows it.
  const want = useRef<ScrollAnchor | null>(null);
  const asked = useRef(0);
  const wheel = useRef(0);
  // The program's mouse: the button held down and the cell it was last reported
  // at, and the cell a plain move was last reported at.
  const held = useRef<{ button: number; x: number; y: number; session: number; modes: Modes } | null>(null);
  const over = useRef<{ x: number; y: number } | null>(null);
  // The pointer the last press came from: only a mouse's is the program's.
  const byMouse = useRef(false);
  /**
   * The held button comes up, at `at` or where it was last seen. The release
   * goes to the session that had the press, as that session asked for it,
   * whatever is open by now: a button is never left down in a program.
   */
  const letGo = (at?: { x: number; y: number } | null, keys?: { altKey: boolean; ctrlKey: boolean }) => {
    const down = held.current;
    if (!down) return;
    held.current = null;
    const data = mouseBytes(
      { action: "release", button: down.button, x: (at ?? down).x, y: (at ?? down).y, alt: keys?.altKey, ctrl: keys?.ctrlKey },
      down.modes,
    );
    if (data) sendBytesTo(down.session, data);
  };

  const toLive = () => {
    asked.current++;
    want.current = null;
    setBack(null);
  };
  const fetchBack = async (anchor: ScrollAnchor) => {
    const n = ++asked.current;
    const page = await scrollback(anchor);
    if (n !== asked.current) return;
    if (!page || page.from_top >= page.history) return toLive();
    want.current = { from_top: page.from_top };
    setBack({ fromTop: page.from_top, history: page.history, rows: page.rows });
  };
  /** Scrolls the history by `lines`, up when negative. */
  const scrollLines = (lines: number) => {
    const at = want.current;
    let next: ScrollAnchor;
    if (!at) {
      if (lines >= 0) return;
      next = { up: -lines };
    } else if ("up" in at) {
      if (at.up - lines <= 0) return toLive();
      next = { up: at.up - lines };
    } else {
      next = { from_top: Math.max(0, at.from_top + lines) };
    }
    want.current = next;
    void fetchBack(next);
  };
  // The handlers below are set up once per focus; they reach the latest of these.
  const now = useRef({ back, toLive, scrollLines, agent, rows: screen?.rows.length ?? 0, cols: screen?.cols ?? 0 });
  now.current = { back, toLive, scrollLines, agent, rows: screen?.rows.length ?? 0, cols: screen?.cols ?? 0 };
  // The session on show and its modes, for what's owed to it once another is open.
  const shown = useRef<{ id: number; modes: Modes } | null>(null);
  if (screen && openId !== null) shown.current = { id: openId, modes: screen.modes };

  // Another session's history is not this one's.
  useEffect(() => {
    now.current.toLive();
    stick.current = true;
    // A button held in the session you left comes up there, not here.
    letGo();
    over.current = null;
  }, [openId]);

  // Follow new output unless you scrolled up to read.
  const onScroll = () => {
    const el = ref.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [screen, view, ref]);

  // History that ends inside the live screen shows rows that are still changing:
  // fetched again when the screen changes, not when the fetch itself lands.
  useEffect(() => {
    const at = now.current.back;
    if (!at || at.fromTop + at.rows.length <= at.history) return;
    const timer = setTimeout(() => {
      if (want.current) void fetchBack(want.current);
    }, REFRESH_MS);
    return () => clearTimeout(timer);
  }, [screen]);

  const keys = typing && live && desktop;
  const ready = !!screen;
  useEffect(() => {
    if (!keys) return;
    const send = (data: number[] | null) => {
      if (!data) return;
      if (now.current.back) now.current.toLive();
      sendBytes(data);
    };
    const focusField = () => field.current?.focus({ preventScroll: true });

    const onKey = (e: KeyboardEvent) => {
      // An input method is mid-word: its keys are its own until it commits.
      if (e.isComposing || e.keyCode === 229) return;
      const pressed = {
        key: e.key,
        code: e.code,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        metaKey: e.metaKey,
        altGraph: e.getModifierState("AltGraph"),
      };
      // Moving between sessions is the app's, here as in the TUI.
      if (shellChord(pressed, MAC)) return;
      // The session you moved to hasn't shown its screen: what it asked of its
      // keys isn't known yet, so nothing is typed blind.
      if (!ready) {
        if (!e.metaKey) e.preventDefault();
        return;
      }
      const modes = openModes();
      const chord = e.ctrlKey && e.shiftKey && !e.altKey;
      // Ctrl+C is the program's, so copy and paste take Shift, as in a terminal.
      if (chord && e.code === "KeyC") {
        e.preventDefault();
        copySelection(now.current.cols);
        return;
      }
      if ((chord || e.metaKey) && e.code === "KeyV") return;
      // The one key that isn't the program's: the way out without a mouse.
      if (chord && e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        onTyping(false);
        return;
      }
      const plainShift = e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
      if (plainShift && (e.key === "PageUp" || e.key === "PageDown") && !modes?.alt_screen) {
        e.preventDefault();
        const page = Math.max(1, now.current.rows - 1);
        now.current.scrollLines(e.key === "PageUp" ? -page : page);
        return;
      }
      const data = encodeKey(pressed, modes, { mac: MAC, agent: now.current.agent });
      if (!data) {
        // A dead key or an input method starting: what it types lands in the field.
        if (!e.metaKey && !e.ctrlKey) focusField();
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      send(data);
    };

    const onPaste = (e: ClipboardEvent) => {
      const clip = e.clipboardData;
      if (!clip) return;
      e.preventDefault();
      // Without the modes a paste would go in unmarked and run line by line.
      if (!ready) return;
      const modes = openModes();
      if (clip.files.length) {
        // A screenshot or a copied file goes up, and its path is pasted.
        for (const file of clip.files) {
          sendFile(file).then(
            (path) => send([...pathBytes(path, openModes()), 0x20]),
            (err: Error) => toast(`${file.name}: ${err.message}`),
          );
        }
        return;
      }
      const text = clip.getData("text/plain");
      if (text) send(pasteBytes(text, modes));
    };

    // Focus is this session's to hear about, even if another is open by the time
    // the keyboard is given back. The keyboard can follow you to another session
    // (Alt-J): the one you left hears it lose focus, the next hears it gain it
    // once its screen, and so what it asked for, is here.
    const session = useApp.getState().openId;
    const report = (focused: boolean) => {
      if (session === null) return;
      const open = useApp.getState().openId === session;
      const modes = open ? openModes() : shown.current?.id === session ? shown.current.modes : undefined;
      const data = focusBytes(focused, modes);
      if (data) sendBytesTo(session, data);
    };
    const onFocus = () => report(true);
    const onBlur = () => report(false);
    // A click anywhere else (the composer, the header) takes the keyboard back.
    const onPointer = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onTyping(false);
    };

    window.addEventListener("keydown", onKey, true);
    window.addEventListener("paste", onPaste);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    document.addEventListener("pointerdown", onPointer, true);
    if (ready) report(true);
    focusField();
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("paste", onPaste);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("pointerdown", onPointer, true);
      // A session whose screen hadn't come was never told it had the keyboard.
      if (ready) report(false);
      field.current?.blur();
    };
  }, [keys, ref, onTyping, openId, ready]);

  // While you type in it, the session is as big as the pane, like any terminal.
  const fit = keys && view === "grid" && !!screen;
  useLayoutEffect(() => {
    if (!fit) return;
    const el = ref.current;
    if (!el) return;
    let sent = "";
    const run = () => {
      if (!pre.current || !probe.current) return;
      const css = getComputedStyle(pre.current);
      const px = (v: string) => parseFloat(v) || 0;
      // The whole box: once the grid fits, neither scrollbar is left to take room.
      const size = fitSize(
        el.offsetWidth - px(css.paddingLeft) - px(css.paddingRight),
        el.offsetHeight - px(css.paddingTop) - px(css.paddingBottom),
        probe.current.getBoundingClientRect().width / PROBE,
        px(css.lineHeight),
      );
      if (!size || sent === `${size.cols}x${size.rows}`) return;
      sent = `${size.cols}x${size.rows}`;
      resizeOpen(size);
    };
    run();
    let timer: number | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = window.setTimeout(run, RESIZE_MS);
    });
    observer.observe(el);
    return () => {
      clearTimeout(timer);
      observer.disconnect();
    };
  }, [fit, ref]);

  if (!screen) return <div ref={ref} className="screen loading">Opening…</div>;

  const rows = back ? back.rows : screen.rows;
  const modes = screen.modes;

  // The program asked for the mouse, and the terminal has the keyboard: clicks,
  // drags and the wheel over the live grid are the program's. Shift keeps them
  // the browser's, to select text, as in a terminal.
  const mouse = keys && view === "grid" && !back && wantsMouse(modes);
  const cellOf = (e: React.MouseEvent): { x: number; y: number } | null => {
    if (!pre.current || !probe.current) return null;
    const css = getComputedStyle(pre.current);
    const box = pre.current.getBoundingClientRect();
    return cellAt(
      e.clientX - box.left - (parseFloat(css.paddingLeft) || 0),
      e.clientY - box.top - (parseFloat(css.paddingTop) || 0),
      probe.current.getBoundingClientRect().width / PROBE,
      parseFloat(css.lineHeight) || 17,
      screen.cols,
      screen.rows.length,
    );
  };
  const report = (action: MouseAction, at: { x: number; y: number }, e: React.MouseEvent, button = e.button) => {
    const data = mouseBytes({ action, button, ...at, alt: e.altKey, ctrl: e.ctrlKey }, modes);
    if (data) sendBytes(data);
  };

  const onWheel = (e: React.WheelEvent) => {
    const el = ref.current;
    if (!desktop || !el || e.ctrlKey || !e.deltaY) return;
    const line = parseFloat(getComputedStyle(pre.current ?? el).lineHeight) || 17;
    // What the pane can scroll itself comes first, as below: the wheel is never
    // both the pane's and the program's.
    const scrolls = e.deltaY < 0 ? el.scrollTop > 0 : el.scrollTop + el.clientHeight < el.scrollHeight - 1;
    if (mouse && !e.shiftKey && !scrolls) {
      wheel.current += e.deltaMode === 0 ? e.deltaY / line : e.deltaY;
      const turns = Math.trunc(wheel.current);
      wheel.current -= turns;
      // One report a line, as a terminal sends them; a flick isn't a hundred.
      const at = cellOf(e);
      for (let i = 0; at && i < Math.min(Math.abs(turns), 10); i++) report(turns < 0 ? "wheel-up" : "wheel-down", at, e);
      return;
    }
    // Past the pane's own edge, the history.
    if (scrolls) return;
    wheel.current += e.deltaMode === 0 ? e.deltaY / line : e.deltaY;
    const lines = Math.trunc(wheel.current);
    if (!lines) return;
    wheel.current -= lines;
    if (modes.alt_screen) {
      // A full-screen program has no history; it may take the wheel as arrows.
      const data = keys ? wheelBytes(lines, modes) : null;
      if (data) sendBytes(data);
    } else {
      scrollLines(lines);
    }
  };

  const lines: Run[][] | null = view === "reflow" ? reflow({ ...screen, rows }) : null;
  return (
    <div
      ref={ref}
      className={`screen ${view}${keys ? " typing" : ""}`}
      title={
        mouse
          ? "The program has the mouse: hold Shift to select text. Ctrl+Shift+Enter gives the keyboard back"
          : keys
            ? "Ctrl+Shift+Enter gives the keyboard back"
            : undefined
      }
      onScroll={onScroll}
      onWheel={onWheel}
      onPointerDown={(e) => {
        byMouse.current = e.pointerType === "mouse";
        // The click that gives the terminal the keyboard is only that.
        if (desktop && live && e.button === 0 && !typing) return onTyping(true);
        if (!mouse || e.shiftKey || e.button > 2 || !byMouse.current || openId === null) return;
        const at = cellOf(e);
        if (!at) return;
        // Text selected with Shift before isn't the program's: its highlight
        // would sit on whatever the click makes the program draw.
        getSelection()?.removeAllRanges();
        letGo();
        report("press", at, e);
        held.current = { button: e.button, ...at, session: openId, modes };
        // A drag that leaves the pane still ends here.
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const down = held.current;
        // The button came up where no event told of it (another window, a
        // broken grab): it comes up now.
        if (down && e.buttons === 0) return letGo(cellOf(e), e);
        if (!mouse || e.pointerType !== "mouse") return;
        if (!down && (!modes.mouse_motion || e.shiftKey)) return;
        // One report a cell, not one a pixel.
        const was = down ?? over.current;
        const at = cellOf(e);
        if (!at || (was && was.x === at.x && was.y === at.y)) return;
        report(down ? "drag" : "move", at, e, down?.button ?? 0);
        if (down) held.current = { ...down, ...at };
        else over.current = at;
      }}
      onPointerUp={(e) => letGo(cellOf(e), e)}
      onPointerCancel={() => letGo()}
      onLostPointerCapture={() => letGo()}
      // The program's click isn't the start of a selection, its right button
      // isn't the browser's menu, and its middle button doesn't paste. A pen or
      // a finger is left alone: only a mouse is reported.
      onMouseDown={(e) => {
        if (mouse && byMouse.current && !e.shiftKey && e.button <= 2) e.preventDefault();
      }}
      onAuxClick={(e) => {
        if (mouse && byMouse.current && !e.shiftKey) e.preventDefault();
      }}
      onContextMenu={(e) => {
        if (mouse && byMouse.current && !e.shiftKey) e.preventDefault();
      }}
      onMouseUp={(e) => {
        if (mouse && byMouse.current && !e.shiftKey && e.button === 1) e.preventDefault();
        // The field holds the keyboard for input methods; a selection stays put.
        if (keys && getSelection()?.isCollapsed) field.current?.focus({ preventScroll: true });
      }}
      onCopy={(e) => {
        const text = view === "grid" ? selectedText(screen.cols) : "";
        if (!text) return;
        e.clipboardData.setData("text/plain", text);
        e.preventDefault();
      }}
    >
      <pre ref={pre} style={view === "grid" ? { width: `${screen.cols}ch` } : undefined}>
        {lines
          ? lines.map((runs, i) => <Line key={i} runs={runs} />)
          : rows.map((row, i) => <GridLine key={i} row={row} />)}
        {view === "grid" && !back && screen.cursor.visible && (
          <span className={`cursor cursor-${screen.cursor.shape ?? "block"}`} style={cell(screen.cursor)} />
        )}
        {view === "grid" && (
          <span ref={probe} className="probe" aria-hidden>
            {"0".repeat(PROBE)}
          </span>
        )}
        {keys && (
          <TypingField field={field} at={view === "grid" && !back ? screen.cursor : null} onLive={toLive} />
        )}
      </pre>
      {back ? (
        <button className="screen-note" onClick={toLive}>
          ↓ Live · {back.history - back.fromTop} lines up
        </button>
      ) : (
        desktop && live && !typing && <span className="screen-note screen-note-quiet">Click to type</span>
      )}
    </div>
  );
}

/** Where a cell sits in the grid's `<pre>`, by its padding and the cell's size. */
function cell(at: Cursor): React.CSSProperties {
  return { left: `calc(var(--screen-pad) + ${at.x}ch)`, top: `calc(var(--screen-pad) + ${at.y} * var(--screen-line))` };
}

function Line({ runs, wrapped }: { runs: Run[]; wrapped?: boolean }) {
  return (
    <div className="line" data-wrapped={wrapped || undefined}>
      {runs.length === 0
        ? " "
        : runs.map((r, j) => (
            <span key={j} style={cssStyle(r.style)}>
              {r.text}
            </span>
          ))}
    </div>
  );
}

/** A grid row. An update replaces only the rows it changed, so the rest don't render again. */
const GridLine = memo(function GridLine({ row }: { row: Row }) {
  return <Line runs={rowRuns(row)} wrapped={row.wrapped} />;
});

/**
 * Where typing lands that a key press can't carry: an input method's committed
 * word, a dead key's accented letter. It sits on the cursor, so the input
 * method's candidates open there.
 */
function TypingField({
  field,
  at,
  onLive,
}: {
  field: RefObject<HTMLTextAreaElement | null>;
  at: Cursor | null;
  onLive: () => void;
}) {
  const enc = new TextEncoder();
  const commit = (text: string) => {
    if (!text) return;
    onLive();
    sendBytes([...enc.encode(text)]);
  };
  // Browsers differ on whether an input event follows the commit, and on what
  // they call it; until the field is cleared, whatever follows is that same text.
  const settling = useRef(false);
  const clear = () => {
    settling.current = false;
    if (field.current) field.current.value = "";
  };
  return (
    <textarea
      ref={field}
      className="typing-field"
      style={at ? cell(at) : undefined}
      aria-label="Terminal input"
      autoCapitalize="off"
      autoCorrect="off"
      autoComplete="off"
      spellCheck={false}
      rows={1}
      onCompositionEnd={(e) => {
        commit(e.data);
        settling.current = true;
        setTimeout(clear, 0);
      }}
      onInput={(e) => {
        // Composing text is sent once, when it's committed.
        if ((e.nativeEvent as InputEvent).isComposing || settling.current) return;
        commit(e.currentTarget.value);
        clear();
      }}
    />
  );
}

/** Copies what's selected, by the clipboard API or, on a plain-http page, the old command. */
function copySelection(cols: number): void {
  const text = selectedText(cols);
  if (!text) return;
  if (navigator.clipboard) void navigator.clipboard.writeText(text).catch(() => document.execCommand("copy"));
  else document.execCommand("copy");
}

/**
 * The selection as a terminal copies it. The browser's own text has a line break
 * after every row; read row by row, a row the program wrapped joins the next.
 */
function selectedText(cols: number): string {
  const selection = getSelection();
  if (!selection || selection.isCollapsed || !selection.rangeCount) return "";
  const range = selection.getRangeAt(0);
  const rows: CopiedRow[] = [];
  const lines = document.querySelectorAll<HTMLElement>(".screen.grid .line");
  for (const line of lines) {
    if (!range.intersectsNode(line)) continue;
    const part = document.createRange();
    part.selectNodeContents(line);
    if (range.compareBoundaryPoints(Range.START_TO_START, part) > 0) part.setStart(range.startContainer, range.startOffset);
    if (range.compareBoundaryPoints(Range.END_TO_END, part) < 0) part.setEnd(range.endContainer, range.endOffset);
    // A selection that only touches a row's edge takes nothing from it.
    if (part.collapsed) continue;
    rows.push({
      text: part.toString(),
      wrapped: line.dataset.wrapped !== undefined,
      missing: cols - textWidth(line.textContent ?? ""),
    });
  }
  // Not the grid's rows (the selection is somewhere else on the page): as the browser has it.
  return rows.length ? copiedRows(rows) : copiedText(selection.toString());
}
