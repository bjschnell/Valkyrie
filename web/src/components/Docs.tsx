import { useEffect, useMemo, useState } from "react";
import type { Doc } from "../proto";
import { body, grouped, resolver, validKey } from "../lib/docs";
import { clock } from "../lib/chat";
import { allDocs, docsSeen, openDoc, useApp } from "../store";
import { Back } from "./icons";
import { Markdown } from "./Markdown";

const go = (hash: string) => {
  location.hash = hash;
};

/** q or Esc goes to `to`, when you aren't typing. */
function useBack(to: string) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        go(to);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [to]);
}

/** Who wrote it and from where, in a line. */
function byline(d: Doc): string {
  return [d.by === "human" ? "you" : d.by, d.session, clock(d.created) || null].filter(Boolean).join(" · ");
}

/**
 * Docs agents wrote for you (ADR-0021 §5): agent-level ones, then each
 * repository's, newest first. Opening the list marks what's in it read.
 */
export function DocsList() {
  const status = useApp((s) => s.status);
  const [docs, setDocs] = useState<Doc[] | null>(null);
  const [failed, setFailed] = useState(false);
  useBack("#/");

  useEffect(() => {
    if (status !== "open") return;
    let live = true;
    void allDocs().then((got) => {
      if (!live) return;
      setFailed(got === null);
      if (!got) return;
      // The marks show what was new before this visit; then it's all read.
      setDocs(got.docs);
      docsSeen();
    });
    return () => {
      live = false;
    };
  }, [status]);

  const groups = docs ? grouped(docs) : [];
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => go("#/")}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Docs</div>
        </div>
      </header>
      <main className="inbox">
        {docs === null && <div className="empty">{failed ? "Couldn't load the docs." : "Loading…"}</div>}
        {docs !== null && docs.length === 0 && (
          <div className="empty">No docs yet. Agents write plans, research and reports here.</div>
        )}
        {groups.map(([group, ds]) => (
          <section key={group || "-"}>
            <h2 className="section-title">
              {group || "Agent-level"} <span className="count">{ds.length}</span>
            </h2>
            <div className="list">
              {ds.map((d) => (
                <button
                  key={d.key || d.path}
                  className="row doc-row"
                  disabled={!d.key}
                  onClick={() => d.key && go(`#/docs/${encodeURIComponent(d.key)}`)}
                >
                  <span className={`kind-chip kind-${d.kind}`}>{d.kind}</span>
                  <span className="row-main">
                    <span className="row-name">{d.title || d.path}</span>
                    <span className="row-sub">{byline(d)}</span>
                  </span>
                  {d.unread && <span className="doc-unread" aria-label="unread" />}
                </button>
              ))}
            </div>
          </section>
        ))}
      </main>
    </div>
  );
}

/** One doc, read-only, with `[[wikilinks]]` to the others. */
export function DocView({ docKey }: { docKey: string }) {
  const status = useApp((s) => s.status);
  const [doc, setDoc] = useState<Doc | null>(null);
  const [list, setList] = useState<Doc[]>([]);
  const [error, setError] = useState<string | null>(null);
  useBack("#/docs");

  useEffect(() => {
    if (status !== "open") return;
    if (!validKey(docKey)) {
      setError("That isn't a doc.");
      return;
    }
    let live = true;
    setDoc(null);
    setError(null);
    openDoc(docKey).then(
      (d) => live && setDoc(d),
      (e: Error) => live && setError(e.message),
    );
    void allDocs().then((got) => live && got && setList(got.docs));
    return () => {
      live = false;
    };
  }, [status, docKey]);

  const wiki = useMemo(() => resolver(list), [list]);
  const text = useMemo(() => (doc?.text ? body(doc.text) : ""), [doc]);
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back to docs" onClick={() => go("#/docs")}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">{doc?.title || "Doc"}</div>
          {doc && <div className="title-sub">{byline(doc)}</div>}
        </div>
      </header>
      <main className="inbox doc-page">
        {error && <div className="empty">{error}</div>}
        {!doc && !error && <div className="empty">Loading…</div>}
        {doc && (
          <>
            <div className="doc-meta">
              <span className={`kind-chip kind-${doc.kind}`}>{doc.kind}</span>
              {doc.item ? <span>backlog #{doc.item}</span> : null}
              {doc.commit ? <code>{doc.commit.slice(0, 8)}</code> : null}
              <span className="doc-path">{doc.path}</span>
              {doc.id === 0 && <span>yours, read-only</span>}
            </div>
            {doc.edited?.length ? <div className="doc-edited">edited by {doc.edited.join("; ")}</div> : null}
            <Markdown text={text} className="doc-body" wiki={wiki} />
          </>
        )}
      </main>
    </div>
  );
}
