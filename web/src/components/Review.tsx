import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { getJson, type Change, type FileDiff, type Review } from "../lib/api";
import { diffRows, splitPath } from "../lib/diff";
import { markSeen, sendTo, toast, useApp } from "../store";
import { Back, Chevron, Send } from "./icons";
import { MicButton } from "./Mic";

const BADGE: Record<Change, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
};

/** Replies to the agent, one tap each. */
const QUICK = [
  { label: "Commit it", text: "Looks good. Commit these changes." },
  { label: "Run the tests", text: "Run the tests and fix anything that fails." },
  { label: "Explain", text: "Walk me through these changes and why you made them." },
];

/** Files opened at first, in order, until this many changed lines are showing. */
const OPEN_LINES = 150;

function firstOpen(files: FileDiff[]): Set<string> {
  const open = new Set<string>();
  let lines = 0;
  for (const f of files) {
    lines += f.added + f.removed;
    if (lines > OPEN_LINES && open.size) break;
    open.add(f.path);
  }
  return open;
}

/**
 * What an agent changed, on a phone (DESIGN §8.8): its repo's diff since the last
 * commit, untracked files included, one card per file with line numbers. From
 * here a reply goes straight to the agent: a quick one, or your own.
 */
export function ReviewView({ id }: { id: number }) {
  const token = useApp((s) => s.token);
  const info = useApp((s) => s.sessions.find((x) => x.id === id));
  const [review, setReview] = useState<Review | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [reply, setReply] = useState("");
  const area = useRef<HTMLTextAreaElement>(null);
  const cards = useRef<(HTMLElement | null)[]>([]);
  const focus = useRef(-1);

  const load = useCallback(async () => {
    if (!token) return;
    setLoading(true);
    try {
      const r = await getJson<Review>(token, `/api/review/${id}`);
      setReview(r);
      setError(null);
      setOpen((was) => (was.size ? was : firstOpen(r.files)));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [token, id]);

  useEffect(() => {
    void load();
  }, [load]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
  }, [reply]);

  const toggle = (path: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const send = (text: string) => {
    if (!sendTo(id, text)) return;
    setReply("");
    navigator.vibrate?.(10);
    toast("Sent ✓");
    location.hash = `#/s/${id}`;
  };

  // j/k move between files, o opens one, r reads the diff again, q goes back.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      const files = review?.files ?? [];
      const move = (by: number) => {
        focus.current = Math.max(0, Math.min(files.length - 1, focus.current + by));
        cards.current[focus.current]?.scrollIntoView({ block: "start", behavior: "smooth" });
      };
      const actions: Record<string, () => void> = {
        j: () => move(1),
        k: () => move(-1),
        o: () => files[focus.current] && toggle(files[focus.current].path),
        r: () => void load(),
        i: () => area.current?.focus(),
        q: () => (location.hash = `#/s/${id}`),
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

  const totals = useMemo(() => {
    const files = review?.files ?? [];
    return {
      added: files.reduce((n, f) => n + f.added, 0),
      removed: files.reduce((n, f) => n + f.removed, 0),
    };
  }, [review]);
  const unseen = info && info.status.state === "review_ready" && !info.status.seen;

  return (
    <div className="page session review-page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => (location.hash = `#/s/${id}`)}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Changes · {info?.name ?? `#${id}`}</div>
          <div className="title-sub">
            {review ? (
              <>
                {review.branch && <span className="branch">{review.branch}</span>}
                <span>
                  {review.files.length} file{review.files.length === 1 ? "" : "s"}
                </span>
                <span className="diff-add">+{totals.added}</span>
                <span className="diff-del">−{totals.removed}</span>
              </>
            ) : (
              "Reading the diff…"
            )}
          </div>
        </div>
        <button className="ghost" onClick={() => void load()} disabled={loading}>
          {loading ? "…" : "Refresh"}
        </button>
      </header>

      <main className="review">
        {error && <div className="review-note">{error.includes("not a git") ? "This session's folder isn't a git repository." : error}</div>}
        {review && review.files.length === 0 && <div className="review-note">No changes since the last commit.</div>}
        {review?.files.map((f, i) => (
          <FileCard
            key={f.path}
            file={f}
            open={open.has(f.path)}
            onToggle={() => toggle(f.path)}
            cardRef={(el) => {
              cards.current[i] = el;
            }}
          />
        ))}
        {review && review.omitted > 0 && (
          <div className="review-note">
            {review.omitted} more file{review.omitted === 1 ? "" : "s"} not shown; the change is too big for one look.
          </div>
        )}
      </main>

      <footer className="dock">
        <div className="quick">
          {unseen && (
            <button className="choice" onClick={() => info && void markSeen({ session: id, name: info.name, cwd: info.cwd, status: info.status })}>
              Mark seen
            </button>
          )}
          {QUICK.map((q) => (
            <button key={q.label} className="choice" onClick={() => send(q.text)}>
              {q.label}
            </button>
          ))}
        </div>
        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            send(reply);
          }}
        >
          <textarea
            ref={area}
            rows={1}
            value={reply}
            placeholder="Ask for changes"
            onChange={(e) => setReply(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && window.matchMedia("(pointer: fine)").matches) {
                e.preventDefault();
                send(reply);
              }
              if (e.key === "Escape") area.current?.blur();
            }}
          />
          <MicButton value={reply} onChange={setReply} />
          <button className="send" aria-label="Send" disabled={!reply.trim()}>
            <Send />
          </button>
        </form>
      </footer>
    </div>
  );
}

function FileCard({
  file,
  open,
  onToggle,
  cardRef,
}: {
  file: FileDiff;
  open: boolean;
  onToggle: () => void;
  cardRef: (el: HTMLElement | null) => void;
}) {
  const { name, dir } = splitPath(file.path);
  const rows = useMemo(() => (open ? diffRows(file.patch) : []), [open, file.patch]);
  return (
    <section ref={cardRef} className={`file-card ${open ? "open" : ""}`}>
      <button className="file-head" onClick={onToggle} aria-expanded={open}>
        <Chevron />
        <span className={`badge badge-${file.status}`} title={file.status}>
          {BADGE[file.status]}
        </span>
        <span className="file-path">
          <span className="file-name">{name}</span>
          {dir && <span className="file-dir"> {dir}</span>}
          {file.from && <span className="file-dir"> from {file.from}</span>}
        </span>
        <span className="diff-add">+{file.added}</span>
        <span className="diff-del">−{file.removed}</span>
      </button>
      {open && (
        <div className="file-body">
          {file.binary ? (
            <div className="review-note">Binary file.</div>
          ) : rows.length === 0 ? (
            <div className="review-note">{file.status === "renamed" ? "Renamed, unchanged." : "No text changes."}</div>
          ) : (
            <div className="hunks" role="table">
              {rows.map((r, i) =>
                r.kind === "hunk" ? (
                  <div key={i} className="drow hunk">
                    <span className="gutter" />
                    <span className="code">{r.text}</span>
                  </div>
                ) : (
                  <div key={i} className={`drow ${r.kind}`}>
                    <span className="gutter">{r.kind === "del" ? r.old : (r.new ?? "")}</span>
                    <span className="code">{r.text || " "}</span>
                  </div>
                ),
              )}
            </div>
          )}
          {file.cut && <div className="review-note">Cut here: the rest is too long to show.</div>}
        </div>
      )}
    </section>
  );
}
