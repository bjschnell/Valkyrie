import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import type { Doc, SessionFile, SessionId } from "../proto";
import { allDocs, openDoc, readSessionFile } from "../store";
import { isText, linked } from "../lib/files";
import { body, resolver, validKey } from "../lib/docs";
import { clock } from "../lib/chat";
import { docHash } from "../lib/routes";
import { Back } from "./icons";
import { Loading } from "./bits";
import { Markdown } from "./Markdown";

/** What the pane shows: a doc by its key, or a file of the session's by its path. */
export type Reading = { doc: string } | { file: string };

/** One row of the list: a doc, or a file. */
type Entry = { doc: Doc } | { file: SessionFile };

/**
 * The docs a session wrote, read beside it (backlog #311), and the markdown and
 * text files its branch changed (ADR-0021 item 9): the list, then one, without
 * leaving the session. Beside the chat or terminal on a desktop, over it on a
 * phone. Comments, editing and history stay on the Docs page, a link away.
 * Files are read-only; the daemon checks every one.
 *
 * Keys, while it's open and you aren't typing: j and k pick (in a doc, scroll),
 * Enter reads, Esc goes back to the list, then closes it.
 */
export function SessionDocs({
  session,
  docs,
  files,
  failed,
  reading,
  onRead,
  onClose,
}: {
  session: SessionId;
  /** The session's docs, null while they load. */
  docs: Doc[] | null;
  /** Its markdown and text files, null while they load. */
  files: SessionFile[] | null;
  /** Why they couldn't be listed, if they couldn't. */
  failed: string | null;
  /** What's on show; null for the list. */
  reading: Reading | null;
  onRead: (what: Reading | null) => void;
  onClose: () => void;
}) {
  const [picked, setPicked] = useState(0);
  const list: Entry[] = useMemo(
    () => [...(docs ?? []).map((doc) => ({ doc })), ...(files ?? []).map((file) => ({ file }))],
    [docs, files],
  );
  // A reload with fewer keeps the highlight on one.
  useEffect(() => setPicked((p) => Math.min(p, Math.max(0, list.length - 1))), [list.length]);
  // Back from one goes to the list only when there's a list to go back to.
  const only = list.length === 1 ? entryReading(list[0]) : null;
  const hasList = list.length > 1 || (only !== null && !same(only, reading));
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
      else if (reading === null && e.key === "Enter" && list[picked] && entryReading(list[picked])) {
        onRead(entryReading(list[picked]));
      }
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

  const nDocs = docs?.length ?? 0;
  const loading = docs === null || files === null;
  return (
    <aside ref={pane} className="session-docs" aria-label="Docs and files from this session">
      {reading !== null && "doc" in reading ? (
        <Reader
          docKey={reading.doc}
          onBack={back}
          backLabel={hasList ? "Back to this session's docs" : "Close"}
          onRead={(key) => onRead({ doc: key })}
        />
      ) : reading !== null ? (
        <FileReader
          session={session}
          path={reading.file}
          onBack={back}
          backLabel={hasList ? "Back to this session's docs" : "Close"}
          onRead={(path) => onRead({ file: path })}
        />
      ) : (
        <>
          <header className="session-docs-head">
            <button className="icon-btn" aria-label="Close" onClick={onClose}>
              <Back />
            </button>
            <span className="session-docs-title">From this session</span>
          </header>
          <div className="session-docs-body">
            {failed && docs === null && <p className="muted session-docs-empty">Couldn't list them: {failed}</p>}
            {loading && !failed && <Loading rows={3} />}
            {!loading && list.length === 0 && (
              <p className="muted session-docs-empty">
                This session hasn't written a doc or changed a markdown file yet. Agents write docs with{" "}
                <code>valk doc write</code>.
              </p>
            )}
            {[0, 1].map((part) => {
              const rows = list.map((entry, i) => ({ entry, i })).filter(({ i }) => (part === 0 ? i < nDocs : i >= nDocs));
              if (!rows.length) return null;
              return (
                <div key={part}>
                  {part === 1 && <h3 className="session-docs-section">Files on its branch</h3>}
                  <div className="list">
                    {rows.map(({ entry, i }) =>
                      "doc" in entry ? (
                        <button
                          key={entry.doc.key || entry.doc.path}
                          className={`row doc-row ${i === picked ? "current" : ""}`}
                          disabled={!entry.doc.key}
                          onClick={() => entry.doc.key && onRead({ doc: entry.doc.key })}
                        >
                          <span className="tag">{entry.doc.kind}</span>
                          <span className="row-main">
                            <span className="row-name">{entry.doc.title || entry.doc.path}</span>
                            <span className="row-sub">{clock(entry.doc.created)}</span>
                          </span>
                        </button>
                      ) : (
                        <button
                          key={`file:${entry.file.path}`}
                          className={`row doc-row ${i === picked ? "current" : ""}`}
                          onClick={() => onRead({ file: entry.file.path })}
                        >
                          <span className="tag">{isText(entry.file.path) ? "text" : "md"}</span>
                          <span className="row-main">
                            <span className="row-name">{entry.file.path}</span>
                            {entry.file.modified != null && (
                              <span className="row-sub">{clock(new Date(entry.file.modified * 1000).toISOString())}</span>
                            )}
                          </span>
                        </button>
                      ),
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </>
      )}
    </aside>
  );
}

function entryReading(e: Entry): Reading | null {
  if ("file" in e) return { file: e.file.path };
  return e.doc.key ? { doc: e.doc.key } : null;
}

function same(a: Reading | null, b: Reading | null): boolean {
  if (!a || !b) return a === b;
  return "doc" in a ? "doc" in b && a.doc === b.doc : "file" in b && a.file === b.file;
}

/**
 * One of the session's files, read-only (ADR-0021 item 9). Markdown renders like
 * a doc, and a relative link to another markdown file opens it here, subject to
 * the daemon's checks again. A `.txt` file shows as preformatted text.
 */
function FileReader({
  session,
  path,
  onBack,
  backLabel,
  onRead,
}: {
  session: SessionId;
  path: string;
  onBack: () => void;
  backLabel: string;
  onRead: (path: string) => void;
}) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    setText(null);
    setError(null);
    readSessionFile(session, path).then(
      (got) => {
        if (!live) return;
        setText(got.text);
        scroller.current?.scrollTo({ top: 0 });
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [session, path]);

  const rel = useMemo(() => (href: string) => linked(path, href), [path]);
  const onClick = (e: MouseEvent) => {
    const a = (e.target as HTMLElement).closest<HTMLAnchorElement>("a.md-rel");
    const to = a?.dataset.path;
    if (to) {
      e.preventDefault();
      onRead(to);
    }
  };
  const name = path.split("/").pop() || path;

  return (
    <>
      <header className="session-docs-head">
        <button className="icon-btn" aria-label={backLabel} onClick={onBack}>
          <Back />
        </button>
        <span className="session-docs-title" title={path}>
          {name}
        </span>
      </header>
      <div ref={scroller} className="session-docs-body" onClick={onClick}>
        {error && <p className="muted session-docs-empty">Couldn't open it: {error}</p>}
        {text === null && !error && <Loading rows={6} />}
        {text !== null && (
          <>
            <div className="doc-meta">
              <span className="tag">{isText(path) ? "text" : "md"}</span>
              <span className="doc-path">{path}</span>
            </div>
            {isText(path) ? <pre className="session-file-text">{text}</pre> : <Markdown text={text} rel={rel} />}
          </>
        )}
      </div>
    </>
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
