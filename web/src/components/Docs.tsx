import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CommentState, DeletedDoc, Doc, DocComment, DocVersion } from "../proto";
import { body, countByQuote, grouped, onlyIn, resolver, validKey } from "../lib/docs";
import { clock } from "../lib/chat";
import {
  allDocs,
  commentDoc,
  deleteDoc,
  deletedDocs,
  docComments,
  docHistory,
  docsSeen,
  openDoc,
  resolveComment,
  restoreDoc,
  saveDoc,
  toast,
  undeleteDoc,
  useApp,
} from "../store";
import { useCached } from "../cache";
import { Loading } from "./bits";
import { Back } from "./icons";
import { Markdown, type BlockComments } from "./Markdown";

const go = (hash: string) => {
  location.hash = hash;
};

/** q or Esc goes to `to` (a hash, or a step back within the page), when you aren't typing. */
function useBack(to: string | (() => void)) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        if (typeof to === "string") go(to);
        else to();
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
  // The marks show what was new before this visit; then it's all read.
  const { data, failed } = useCached("docs", allDocs, (got) => docsSeen(got.at));
  const docs = data?.docs ?? null;
  const [gone, setGone] = useState<DeletedDoc[]>([]);
  useBack("#/");

  useEffect(() => {
    if (status !== "open") return;
    let live = true;
    void deletedDocs().then((ds) => live && setGone(ds));
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
        {docs === null && (failed ? <div className="empty">Couldn't load the docs.</div> : <Loading />)}
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
        {gone.length > 0 && <Deleted gone={gone} />}
      </main>
    </div>
  );
}

/**
 * Docs you deleted here, kept for 30 days (the last 50), newest first: folded
 * away, with Put back on each, so a missed Undo isn't the end of it.
 */
function Deleted({ gone }: { gone: DeletedDoc[] }) {
  const [busy, setBusy] = useState<string | null>(null);
  const putBack = async (d: DeletedDoc) => {
    setBusy(d.token);
    try {
      const back = await undeleteDoc(d.token);
      toast(back.path === d.path ? `Put back “${d.title}”.` : `Put back as ${back.path}: a file took its name.`);
      go(`#/docs/${encodeURIComponent(back.key || "")}`);
    } catch (e) {
      toast(`Couldn't put it back: ${(e as Error).message}`);
      setBusy(null);
    }
  };
  return (
    <details className="doc-deleted">
      <summary className="section-title">
        Recently deleted <span className="count">{gone.length}</span>
      </summary>
      <div className="list">
        {gone.map((d) => (
          <div key={d.token} className="doc-version">
            <span className="row doc-version-row">
              <span className="row-main">
                <span className="row-name">{d.title || d.path}</span>
                <span className="row-sub">
                  {d.path} · deleted {when(d.at)}
                </span>
              </span>
            </span>
            <button className="ghost" disabled={busy !== null} onClick={() => void putBack(d)}>
              Put back
            </button>
          </div>
        ))}
      </div>
    </details>
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

/** What the doc page shows under the doc's header. */
type Mode = "read" | "edit" | "history";

/**
 * One doc, with `[[wikilinks]]` to the others. A doc with a valk-id takes
 * comments (ADR-0021 §6): tap a block, or comment on the whole doc, and the
 * session that wrote it hears it at its next prompt. Light editing, history and
 * delete (§7): heavy editing belongs in Obsidian.
 */
export function DocView({ docKey }: { docKey: string }) {
  const status = useApp((s) => s.status);
  const [doc, setDoc] = useState<Doc | null>(null);
  const [list, setList] = useState<Doc[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [comments, setComments] = useState<DocComment[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [mode, setMode] = useState<Mode>("read");
  // In the editor or history, back is the doc itself: the hash stays the same.
  const toRead = useCallback(() => setMode("read"), []);
  useBack(mode === "read" ? "#/docs" : toRead);

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
    setMode("read");
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

  /** A doc the daemon wrote: shown, and under its new key if it got an id. */
  const written = (d: Doc) => {
    setDoc(d);
    setMode("read");
    if (d.key && d.key !== docKey) location.replace(`#/docs/${encodeURIComponent(d.key)}`);
  };

  const remove = async () => {
    if (!doc?.hash) return;
    try {
      const got = await deleteDoc(docKey, doc.hash);
      if ("conflict" in got) {
        setDoc(got.conflict);
        toast("It changed since you opened it. This is the new version: delete again to delete it.");
        return;
      }
      const title = doc.title || doc.path;
      go("#/docs");
      toast(`Deleted “${title}”.`, {
        label: "Undo",
        run: () =>
          void undeleteDoc(got.token).then(
            (d) => go(`#/docs/${encodeURIComponent(d.key || "")}`),
            (e: Error) => toast(`Couldn't put it back: ${e.message}`),
          ),
      });
    } catch (e) {
      toast(`Couldn't delete it: ${(e as Error).message}`);
    }
  };

  const back = () => (mode === "read" ? go("#/docs") : setMode("read"));
  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label={mode === "read" ? "Back to docs" : "Back to the doc"} onClick={back}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">{doc?.title || "Doc"}</div>
          {doc && <div className="title-sub">{byline(doc)}</div>}
        </div>
      </header>
      <main className="inbox doc-page">
        {error && <div className="empty">{error}</div>}
        {!doc && !error && <Loading rows={6} />}
        {doc && (
          <>
            <div className="doc-meta">
              <span className={`kind-chip kind-${doc.kind}`}>{doc.kind}</span>
              {doc.item ? <span>backlog #{doc.item}</span> : null}
              {doc.commit ? <code>{doc.commit.slice(0, 8)}</code> : null}
              <span className="doc-path">{doc.path}</span>
              {doc.id === 0 && <span>yours</span>}
              {mode === "read" && (
                <span className="doc-actions">
                  {canComment && (
                    <button className="ghost" onClick={() => setDraft({ quote: null })}>
                      Comment
                    </button>
                  )}
                  <button className="ghost" disabled={!doc.hash} onClick={() => setMode("edit")}>
                    Edit
                  </button>
                  {canComment && (
                    <button className="ghost" onClick={() => setMode("history")}>
                      History
                    </button>
                  )}
                  <button className="ghost danger" disabled={!doc.hash} onClick={() => void remove()}>
                    Delete
                  </button>
                </span>
              )}
            </div>
            {mode === "edit" && (
              <Editor
                doc={doc}
                docKey={docKey}
                wiki={wiki}
                onDone={(d) => (d ? written(d) : setMode("read"))}
              />
            )}
            {mode === "history" && (
              <History
                doc={doc}
                docKey={docKey}
                onRestored={(d) => {
                  written(d);
                  toast("Restored. What it replaced is in its history.");
                }}
                onChanged={(d) => {
                  setDoc(d);
                  toast("It changed since you opened it: this is the new version.");
                }}
              />
            )}
            {mode === "read" && (
              <>
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

/** A body as lines, the ones `other` lacks marked. */
function Lines({ text, other }: { text: string; other: string }) {
  const marked = useMemo(() => onlyIn(text, other), [text, other]);
  return (
    <pre className="doc-side-text">
      {text.split("\n").map((line, i) => (
        <div key={i} className={marked.has(i) ? "doc-line-new" : undefined}>
          {line || "\u00a0"}
        </div>
      ))}
    </pre>
  );
}

/**
 * Light editing (ADR-0021 §7): the body in a textarea, with a preview, saved
 * with the hash it loaded. If an agent or Obsidian saved in between, the save
 * is refused and both versions are shown side by side.
 */
function Editor({
  doc,
  docKey,
  wiki,
  onDone,
}: {
  doc: Doc;
  docKey: string;
  wiki: (target: string) => string | null;
  onDone: (written: Doc | null) => void;
}) {
  const loaded = useMemo(() => body(doc.text ?? ""), [doc]);
  const [text, setText] = useState(loaded);
  const [base, setBase] = useState(doc.hash ?? "");
  const [preview, setPreview] = useState(false);
  const [theirs, setTheirs] = useState<Doc | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const dirty = text !== loaded;

  const save = async (over: string) => {
    if (busy) return;
    setBusy(true);
    try {
      const got = await saveDoc(docKey, over, text);
      if (got.conflict) {
        setTheirs(got.doc);
        setPreview(false);
      } else {
        toast("Saved.");
        onDone(got.doc);
      }
    } catch (e) {
      toast(`Couldn't save: ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  if (theirs) {
    const now = body(theirs.text ?? "");
    return (
      <section className="doc-editor" aria-label="Both versions">
        <div className="doc-conflict-note">
          Someone saved this doc since you opened it ({lastEdit(theirs)}). Yours wasn't saved. Lines one side has and
          the other doesn't are marked.
        </div>
        <div className="doc-sides">
          <div className="doc-side">
            <h3 className="section-title">Yours</h3>
            <Lines text={text} other={now} />
          </div>
          <div className="doc-side">
            <h3 className="section-title">On disk now</h3>
            <Lines text={now} other={text} />
          </div>
        </div>
        <div className="doc-composer-actions">
          <button className="ghost" onClick={() => onDone(theirs)}>
            Use theirs
          </button>
          <button
            className="ghost"
            onClick={() => {
              setBase(theirs.hash ?? "");
              setTheirs(null);
            }}
          >
            Keep editing
          </button>
          <button className="primary" disabled={busy} onClick={() => void save(theirs.hash ?? "")}>
            Save mine over it
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="doc-editor" aria-label="Edit">
      <div className="doc-editor-tabs" role="tablist">
        <button role="tab" aria-selected={!preview} className={!preview ? "on" : ""} onClick={() => setPreview(false)}>
          Write
        </button>
        <button role="tab" aria-selected={preview} className={preview ? "on" : ""} onClick={() => setPreview(true)}>
          Preview
        </button>
      </div>
      {preview ? (
        <Markdown text={text} className="doc-body doc-preview" wiki={wiki} />
      ) : (
        <div className="field">
          <textarea
            ref={ref}
            className="doc-textarea"
            value={text}
            spellCheck
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                if (dirty) void save(base);
              }
            }}
          />
        </div>
      )}
      <div className="doc-composer-actions">
        <span className="doc-editor-hint">The properties stay as they are. Big edits: Obsidian.</span>
        <button className="ghost" onClick={() => onDone(null)}>
          {dirty ? "Discard" : "Cancel"}
        </button>
        <button className="primary" disabled={!dirty || busy} onClick={() => void save(base)}>
          Save
        </button>
      </div>
    </section>
  );
}

/** Who last touched a doc, from its `edited` lines, or its author. */
function lastEdit(d: Doc): string {
  const last = d.edited?.[d.edited.length - 1];
  const who = (last ?? d.by ?? "").split(", ")[0];
  return who === "human" || !who ? "you, maybe in Obsidian" : who;
}

/** A size, short. */
function kb(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
}

/**
 * A doc's past versions (ADR-0021 §4), newest first: each is the file as it was
 * before an edit replaced it. Tap one to see it; Restore puts it back, and what
 * it replaces joins the list, so a restore is undone the same way.
 */
function History({
  doc,
  docKey,
  onRestored,
  onChanged,
}: {
  doc: Doc;
  docKey: string;
  onRestored: (d: Doc) => void;
  onChanged: (d: Doc) => void;
}) {
  const [versions, setVersions] = useState<DocVersion[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<{ version: string; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    docHistory(docKey).then(
      (got) => live && setVersions(got.versions),
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [docKey, doc.hash]);

  const peek = (version: string) => {
    if (open?.version === version) return setOpen(null);
    docHistory(docKey, version).then(
      (got) => setOpen({ version, text: body(got.text ?? "") }),
      (e: Error) => toast(`Couldn't open it: ${e.message}`),
    );
  };

  const restore = async (version: string) => {
    if (busy || !doc.hash) return;
    setBusy(true);
    try {
      const got = await restoreDoc(docKey, doc.hash, version);
      if (got.conflict) onChanged(got.doc);
      else onRestored(got.doc);
    } catch (e) {
      toast(`Couldn't restore it: ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="doc-history" aria-label="History">
      <h2 className="section-title">
        History <span className="count">{versions?.length ?? ""}</span>
      </h2>
      {error && <div className="empty">{error}</div>}
      {!versions && !error && <div className="empty">Loading…</div>}
      {versions?.length === 0 && (
        <div className="empty">No past versions yet. Each edit keeps the one it replaces, for 30 days.</div>
      )}
      <div className="list">
        {versions?.map((v) => (
          <div key={v.version} className="doc-version">
            <button className="row doc-version-row" aria-expanded={open?.version === v.version} onClick={() => peek(v.version)}>
              <span className="row-main">
                <span className="row-name">{when(v.at)}</span>
                <span className="row-sub">
                  {v.by === "human" || !v.by ? "you" : v.by} · {kb(v.size)}
                </span>
              </span>
            </button>
            <button className="ghost" disabled={busy} onClick={() => void restore(v.version)}>
              Restore
            </button>
            {open?.version === v.version && <pre className="doc-side-text doc-version-text">{open.text}</pre>}
          </div>
        ))}
      </div>
    </section>
  );
}
