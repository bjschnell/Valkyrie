import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import type { Doc } from "../proto";
import { allDocs, openDoc } from "../store";
import { body, resolver, validKey } from "../lib/docs";
import { clock } from "../lib/chat";
import { docHash } from "../lib/routes";
import { Back } from "./icons";
import { Loading } from "./bits";
import { Markdown } from "./Markdown";

/**
 * The docs a session wrote, read beside it (backlog #311): the list, then one
 * doc, without leaving the session. Beside the chat or terminal on a desktop,
 * over it on a phone. Comments, editing and history stay on the Docs page, a
 * link away.
 *
 * Keys, while it's open and you aren't typing: j and k pick (in a doc, scroll),
 * Enter reads, Esc goes back to the list, then closes it.
 */
export function SessionDocs({
  docs,
  failed,
  reading,
  onRead,
  onClose,
}: {
  /** The session's docs, null while they load. */
  docs: Doc[] | null;
  /** Why they couldn't be listed, if they couldn't. */
  failed: string | null;
  /** The doc on show, by key; null for the list. */
  reading: string | null;
  onRead: (key: string | null) => void;
  onClose: () => void;
}) {
  const [picked, setPicked] = useState(0);
  const list = docs ?? [];
  // A reload with fewer docs keeps the highlight on one.
  useEffect(() => setPicked((p) => Math.min(p, Math.max(0, list.length - 1))), [list.length]);
  // Back from a doc goes to the list only when there's a list to go back to.
  const hasList = list.length > 1 || (list.length === 1 && reading !== list[0].key);
  const back = () => (reading !== null && hasList ? onRead(null) : onClose());

  const pane = useRef<HTMLElement>(null);
  const latest = useRef({ list, picked, reading, back, onRead, onClose });
  latest.current = { list, picked, reading, back, onRead, onClose };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target instanceof Element ? e.target : null;
      if (target?.closest("textarea, input, select, [contenteditable]")) return;
      // A sheet over the page ('?') has the keys, Esc above all.
      if (document.querySelector("dialog[open]")) return;
      // Enter on a focused button or link is that control's.
      if (e.key === "Enter" && target?.closest("button, a, [role=button]")) return;
      const { list, picked, reading, back, onRead, onClose } = latest.current;
      const at = (n: number) => setPicked(Math.max(0, Math.min(list.length - 1, n)));
      const by = (dy: number) => pane.current?.querySelector(".session-docs-body")?.scrollBy({ top: dy });
      let done = true;
      if (e.key === "Escape") back();
      else if (e.key === "D") onClose();
      else if (reading !== null && (e.key === "j" || e.key === "k")) by(e.key === "j" ? 80 : -80);
      else if (reading === null && e.key === "j") at(picked + 1);
      else if (reading === null && e.key === "k") at(picked - 1);
      else if (reading === null && e.key === "Enter" && list[picked]?.key) onRead(list[picked].key);
      else done = false;
      if (done) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    };
    // Ahead of the session's own keys, which it stands in for while open.
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, []);

  return (
    <aside ref={pane} className="session-docs" aria-label="Docs from this session">
      {reading !== null ? (
        <Reader docKey={reading} onBack={back} backLabel={hasList ? "Back to this session's docs" : "Close"} onRead={onRead} />
      ) : (
        <>
          <header className="session-docs-head">
            <button className="icon-btn" aria-label="Close" onClick={onClose}>
              <Back />
            </button>
            <span className="session-docs-title">Docs from this session</span>
          </header>
          <div className="session-docs-body">
            {failed && docs === null && <p className="muted session-docs-empty">Couldn't list them: {failed}</p>}
            {docs === null && !failed && <Loading rows={3} />}
            {docs !== null && docs.length === 0 && (
              <p className="muted session-docs-empty">
                This session hasn't written a doc yet. Agents write them with <code>valk doc write</code>.
              </p>
            )}
            <div className="list">
              {list.map((d, i) => (
                <button
                  key={d.key || d.path}
                  className={`row doc-row ${i === picked ? "current" : ""}`}
                  disabled={!d.key}
                  onClick={() => d.key && onRead(d.key)}
                >
                  <span className="tag">{d.kind}</span>
                  <span className="row-main">
                    <span className="row-name">{d.title || d.path}</span>
                    <span className="row-sub">{clock(d.created)}</span>
                  </span>
                </button>
              ))}
            </div>
          </div>
        </>
      )}
    </aside>
  );
}

/** One doc, read-only. A `[[wikilink]]` in it opens its doc here too. */
function Reader({
  docKey,
  onBack,
  backLabel,
  onRead,
}: {
  docKey: string;
  onBack: () => void;
  backLabel: string;
  onRead: (key: string) => void;
}) {
  const [doc, setDoc] = useState<Doc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [all, setAll] = useState<Doc[]>([]);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    setDoc(null);
    setError(null);
    if (!validKey(docKey)) {
      setError("That isn't a doc.");
      return;
    }
    openDoc(docKey).then(
      (d) => {
        if (!live) return;
        setDoc(d);
        scroller.current?.scrollTo({ top: 0 });
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [docKey]);
  // Wikilinks resolve against every doc, as on the Docs page; fetched once.
  useEffect(() => {
    let live = true;
    void allDocs().then((got) => live && got && setAll(got.docs));
    return () => {
      live = false;
    };
  }, []);

  const wiki = useMemo(() => resolver(all), [all]);
  const text = useMemo(() => (doc?.text ? body(doc.text) : ""), [doc]);
  // A wikilink's href is the Docs page's; beside the session it opens here.
  const onClick = (e: MouseEvent) => {
    const a = (e.target as HTMLElement).closest<HTMLAnchorElement>("a.md-wiki");
    const key = a && decodeURIComponent(a.hash.split("/").pop() ?? "");
    if (key && validKey(key)) {
      e.preventDefault();
      onRead(key);
    }
  };

  return (
    <>
      <header className="session-docs-head">
        <button className="icon-btn" aria-label={backLabel} onClick={onBack}>
          <Back />
        </button>
        <span className="session-docs-title">{doc?.title || "Doc"}</span>
        <a className="link session-docs-full" href={docHash(docKey)}>
          Open in Docs
        </a>
      </header>
      <div ref={scroller} className="session-docs-body" onClick={onClick}>
        {error && <p className="muted session-docs-empty">Couldn't open it: {error}</p>}
        {!doc && !error && <Loading rows={6} />}
        {doc && (
          <>
            <div className="doc-meta">
              <span className="tag">{doc.kind}</span>
              {doc.by && <span>{doc.by === "human" ? "you" : doc.by}</span>}
              {doc.created && <span>{clock(doc.created)}</span>}
              <span className="doc-path">{doc.path}</span>
            </div>
            <Markdown text={text} wiki={wiki} />
          </>
        )}
      </div>
    </>
  );
}
