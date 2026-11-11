import { useEffect, useMemo, useRef, useState } from "react";
import type { CommentState, Doc, DocComment } from "../proto";
import { body, countByQuote, grouped, resolver, validKey } from "../lib/docs";
import { clock } from "../lib/chat";
import { allDocs, commentDoc, docComments, docsSeen, openDoc, resolveComment, toast, useApp } from "../store";
import { Back } from "./icons";
import { Markdown, type BlockComments } from "./Markdown";

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
  const who = d.id === 0 ? "yours" : d.by === "human" || !d.by ? "you" : d.by;
  return [who, d.session, clock(d.created) || null].filter(Boolean).join(" · ");
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
      docsSeen(got.at);
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

/** What a comment's state chip says. */
const STATE: Record<CommentState, string> = {
  open: "open",
  sent: "sent",
  answered: "answered",
  resolved: "resolved",
};

/** When, from seconds. */
function when(secs: number): string {
  return clock(new Date(secs * 1000).toISOString());
}

/** The comment being written: on a block, by its quote, or on the whole doc. */
type Draft = { quote: string | null };

/**
 * One doc, read-only, with `[[wikilinks]]` to the others. A doc with a valk-id
 * takes comments (ADR-0021 §6): tap a block, or comment on the whole doc, and
 * the session that wrote it hears it at its next prompt.
 */
export function DocView({ docKey }: { docKey: string }) {
  const status = useApp((s) => s.status);
  const [doc, setDoc] = useState<Doc | null>(null);
  const [list, setList] = useState<Doc[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [comments, setComments] = useState<DocComment[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
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
    setComments([]);
    setDraft(null);
    openDoc(docKey).then(
      (d) => {
        if (!live) return;
        setDoc(d);
        if (d.id > 0)
          docComments(docKey).then(
            (cs) => live && setComments(cs),
            () => undefined,
          );
      },
      (e: Error) => live && setError(e.message),
    );
    void allDocs().then((got) => live && got && setList(got.docs));
    return () => {
      live = false;
    };
  }, [status, docKey]);

  const wiki = useMemo(() => resolver(list), [list]);
  const text = useMemo(() => (doc?.text ? body(doc.text) : ""), [doc]);
  const counts = useMemo(() => countByQuote(comments), [comments]);
  const canComment = !!doc && doc.id > 0;
  const blocks: BlockComments | undefined = canComment
    ? {
        picked: draft?.quote ?? null,
        pick: (quote) => setDraft((d) => (d?.quote === quote ? null : { quote })),
        counts,
      }
    : undefined;

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
              {canComment && (
                <button className="ghost doc-comment-all" onClick={() => setDraft({ quote: null })}>
                  Comment on the doc
                </button>
              )}
            </div>
            {doc.edited?.length ? <div className="doc-edited">edited by {doc.edited.join("; ")}</div> : null}
            {canComment && comments.length === 0 && !draft && (
              <div className="doc-hint">Tap a heading, paragraph, list item or code block to comment on it.</div>
            )}
            <Markdown text={text} className="doc-body" wiki={wiki} comments={blocks} />
            {comments.length > 0 && (
              <CommentList
                comments={comments}
                onResolve={(id) =>
                  resolveComment(docKey, id).then(setComments, (e: Error) => toast(`Couldn't close it: ${e.message}`))
                }
              />
            )}
            {draft && (
              <Composer
                key={draft.quote ?? ""}
                quote={draft.quote}
                onCancel={() => setDraft(null)}
                onSend={async (words) => {
                  try {
                    const got = await commentDoc(docKey, draft.quote, words);
                    setComments(got.comments);
                    setDraft(null);
                    toast(
                      got.to
                        ? `${got.to} hears it at its next prompt.`
                        : "The session that wrote it isn't running: the comment stays open here.",
                    );
                    return true;
                  } catch (e) {
                    toast(`Couldn't comment: ${(e as Error).message}`);
                    return false;
                  }
                }}
              />
            )}
          </>
        )}
      </main>
    </div>
  );
}

/** Writing a comment, pinned to the bottom, with the block it's about. */
function Composer({
  quote,
  onCancel,
  onSend,
}: {
  quote: string | null;
  onCancel: () => void;
  onSend: (text: string) => Promise<boolean>;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const send = async () => {
    if (!text.trim() || busy) return;
    setBusy(true);
    const ok = await onSend(text.trim());
    if (!ok) setBusy(false);
  };
  return (
    <div className="doc-composer" role="dialog" aria-label="Comment">
      <div className="doc-composer-about">{quote ? <q>{quote}</q> : "On the whole doc"}</div>
      <div className="field">
        <textarea
          ref={ref}
          rows={3}
          value={text}
          maxLength={4096}
          placeholder="Your comment, for the agent that wrote it"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void send();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            }
          }}
        />
      </div>
      <div className="doc-composer-actions">
        <button className="ghost" onClick={onCancel}>
          Cancel
        </button>
        <button className="primary" disabled={!text.trim() || busy} onClick={() => void send()}>
          Comment
        </button>
      </div>
    </div>
  );
}

/** The doc's comments, newest first; closed ones last. */
function CommentList({ comments, onResolve }: { comments: DocComment[]; onResolve: (id: number) => void }) {
  const shown = [...comments].sort(
    (a, b) => Number(a.state === "resolved") - Number(b.state === "resolved") || b.at - a.at,
  );
  return (
    <section className="doc-comments">
      <h2 className="section-title">
        Comments <span className="count">{comments.filter((c) => c.state !== "resolved").length}</span>
      </h2>
      {shown.map((c) => (
        <div key={c.id} className={`doc-comment state-${c.state}`}>
          <div className="doc-comment-head">
            <span className={`comment-state comment-${c.state}`}>{STATE[c.state]}</span>
            <span className="doc-comment-when">
              {when(c.at)}
              {c.sent_to ? ` · to ${c.sent_to}` : ""}
            </span>
            {c.state !== "resolved" && (
              <button className="ghost" onClick={() => onResolve(c.id)}>
                Resolve
              </button>
            )}
          </div>
          {c.quote ? <q className="doc-comment-quote">{c.quote}</q> : <div className="doc-comment-quote">On the whole doc</div>}
          <div className="doc-comment-text">{c.text}</div>
        </div>
      ))}
    </section>
  );
}
