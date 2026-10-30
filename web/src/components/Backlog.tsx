import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getJson, type Places } from "../lib/api";
import { COLUMNS, base, byColumn, editChange, factsLine, liveSession, type Lane } from "../lib/backlog";
import { age } from "../lib/time";
import { ITEM_KINDS, type Item, type ItemFacts, type ItemKind } from "../proto";
import { addItem, changeItem, deleteItem, itemFacts, launchItem, noteItem, sendFile, toast, useApp } from "../store";
import { Back, Clip, Plus } from "./icons";
import { MicButton } from "./Mic";

type Agent = "claude" | "codex";
const AGENT_KEY = "valk.backlog.agent";
const FOLDER_KEY = "valk.backlog.folder";
const DRAFT_KEY = "valk.backlog.draft";

const read = (key: string) => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};
const write = (key: string, value: string | null) => {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* kept for this visit only */
  }
};

const back = () => (history.length > 1 ? history.back() : (location.hash = "#/backlog"));

/** The edge color of a card, as the inbox's cards show a session's state. */
const EDGE: Record<Lane, string> = {
  proposed: "item-proposed",
  needs: "state-needs_input",
  running: "state-working",
  review: "state-review_ready",
  backlog: "",
  done: "",
};

/**
 * The backlog (DESIGN §8.12): bugs, features and chores written down for later.
 * Each one starts an agent on itself in a worktree of its own, and its lane
 * follows that session, so nothing here is moved by hand.
 */
export function Backlog() {
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const [showDone, setShowDone] = useState(false);
  const columns = byColumn(items, sessions, boot);
  const open = items.length - columns.done.length;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input, select") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "n" || e.key === "a") {
        e.preventDefault();
        location.hash = "#/backlog/new";
      } else if (e.key === "q") {
        e.preventDefault();
        location.hash = "#/";
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => (location.hash = "#/")}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Backlog</div>
        </div>
        <button className="icon-btn" aria-label="Write something down" onClick={() => (location.hash = "#/backlog/new")}>
          <Plus />
        </button>
      </header>
      <main className="inbox backlog">
        {open === 0 && (
          <div className="calm backlog-calm">
            <div className="calm-title">Nothing written down</div>
            <div className="calm-sub">
              Note a bug or an idea when it comes to you, with a screenshot if you have one. When you're ready, one tap starts an
              agent on it in a worktree of its own.
            </div>
            <button className="primary" onClick={() => (location.hash = "#/backlog/new")}>
              Write something down
            </button>
          </div>
        )}
        {items.length > 0 && (
          <div className="board">
            {COLUMNS.map(({ column, title }) => {
              const cards = columns[column];
              const shut = column === "done" && !showDone;
              return (
                <section key={column} className={`column ${cards.length === 0 ? "column-empty" : ""} ${shut ? "column-shut" : ""}`}>
                  {column === "done" ? (
                    <button className="section-title section-toggle" aria-expanded={showDone} onClick={() => setShowDone(!showDone)}>
                      {title} <span className="count">{cards.length}</span>
                    </button>
                  ) : (
                    <h2 className="section-title">
                      {title} <span className="count">{cards.length}</span>
                    </h2>
                  )}
                  <div className="cards">
                    {cards.slice(0, column === "done" ? 30 : undefined).map(({ item, lane }) => (
                      <ItemCard key={item.id} item={item} lane={lane} />
                    ))}
                    {cards.length === 0 && <div className="column-none">Nothing here</div>}
                  </div>
                </section>
              );
            })}
          </div>
        )}
      </main>
    </div>
  );
}

function ItemCard({ item, lane }: { item: Item; lane: Lane }) {
  const now = useApp((s) => s.now);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [agent, setAgent] = useState<Agent>(() => (read(AGENT_KEY) === "codex" ? "codex" : "claude"));
  const live = liveSession(item, sessions, boot);
  const folder = item.project ? base(item.project) : null;

  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };
  const launch = () =>
    act(async () => {
      try {
        write(AGENT_KEY, agent);
        const id = await launchItem(item.id, agent);
        navigator.vibrate?.(10);
        location.hash = `#/s/${id}`;
      } catch (e) {
        toast(`Couldn't start: ${(e as Error).message}`);
      }
    });
  const status = (s: Item["status"]) => act(() => changeItem(item.id, { do: "status", status: s }));
  // Deleting can't be undone, so it asks twice: the first tap arms it for a moment.
  const [sure, setSure] = useState(false);
  useEffect(() => {
    if (!sure) return;
    const t = setTimeout(() => setSure(false), 5000);
    return () => clearTimeout(t);
  }, [sure]);
  const remove = () => (sure ? act(() => deleteItem(item.id)) : setSure(true));

  // What its checkout holds, read from git when the card opens and as it changes.
  const [facts, setFacts] = useState<ItemFacts | null>(null);
  useEffect(() => {
    if (!expanded || !item.checkout) return setFacts(null);
    let stale = false;
    itemFacts(item.id).then(
      (f) => stale || setFacts(f),
      () => stale || setFacts(null),
    );
    return () => {
      stale = true;
    };
  }, [expanded, item.id, item.checkout, item.updated]);
  const latest = item.notes?.at(-1);
  const [draft, setDraft] = useState("");
  const leave = () =>
    act(async () => {
      if (await noteItem(item.id, draft)) setDraft("");
    });
  const who = (by: string) => (by === "human" ? "you" : by);

  return (
    <div className="card-slot">
      <article className={`card item-card ${EDGE[lane]} ${lane === "done" ? "item-done" : ""}`}>
        <button className="card-head" onClick={() => setExpanded(!expanded)} aria-expanded={expanded}>
          <span className={`kind-chip kind-${item.kind}`}>{item.kind}</span>
          <span className="card-prog">#{item.id}</span>
          <span className="card-prog item-folder">{folder ?? "no folder"}</span>
          <span className="card-age">{lane === "done" ? item.status : age(item.updated * 1000, now)}</span>
        </button>
        <p className="decision-title item-title" onClick={() => setExpanded(!expanded)}>
          {item.title}
        </p>
        {lane === "proposed" && (
          <div className="item-live">
            <strong className="item-needs item-proposed-pill">Proposed</strong>by {item.by}, waiting on you
          </div>
        )}
        {live && (
          <div className="item-live">
            {lane === "needs" && <strong className="item-needs">Needs you</strong>}
            {live.name} · {live.status.state.replace("_", " ")}
          </div>
        )}
        {!expanded && lane === "review" && latest && (
          <p className="item-handoff" onClick={() => setExpanded(true)}>
            <span className="item-handoff-by">{who(latest.by)}:</span> {latest.text}
          </p>
        )}
        {expanded && (
          <>
            {item.body && item.body !== item.title && <p className="decision-body">{item.body}</p>}
            {(item.attachments ?? []).map((a) => (
              <div key={a} className="item-file">
                <Clip /> {base(a)}
              </div>
            ))}
            <div className="decision-by">
              {item.by === "human" ? "written by you" : `written by ${item.by}`}
              {item.branch && <> · {item.branch}</>}
            </div>
            {facts && (
              <div className="item-facts">
                <div className="item-facts-line">{factsLine(facts)}</div>
                {facts.commits.map((c) => (
                  <div key={c} className="item-commit">
                    {c}
                  </div>
                ))}
              </div>
            )}
            {(item.notes ?? []).map((n, i) => (
              <div key={i} className="item-note">
                <div className="item-note-by">
                  {who(n.by)} · {age(n.at * 1000, now)}
                </div>
                <p>{n.text}</p>
              </div>
            ))}
            {lane !== "proposed" && (
              <form
                className="item-note-add"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (draft.trim()) void leave();
                }}
              >
                <input
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  placeholder="Add a note for the next launch"
                  aria-label="Add a note"
                  maxLength={4000}
                />
                <button className="ghost" disabled={busy || !draft.trim()}>
                  Note
                </button>
              </form>
            )}
          </>
        )}
        <div className="card-actions">
          {(expanded || lane === "proposed") && !live && (
            <button className={`ghost no ${sure ? "item-sure" : ""}`} disabled={busy} onClick={() => void remove()}>
              {sure ? "Delete for good" : "Delete"}
            </button>
          )}
          {lane === "proposed" ? (
            <>
              {expanded && (
                <button className="ghost" disabled={busy} onClick={() => (location.hash = `#/backlog/${item.id}`)}>
                  Edit
                </button>
              )}
              <button className="primary" disabled={busy} onClick={() => void status("open")}>
                Accept
              </button>
            </>
          ) : lane === "done" ? (
            <button className="ghost" disabled={busy} onClick={() => void status("open")}>
              Reopen
            </button>
          ) : (
            <>
              {expanded && (
                <button className="ghost no" disabled={busy} onClick={() => void status("dropped")}>
                  Drop
                </button>
              )}
              {expanded && (
                <button className="ghost" disabled={busy} onClick={() => (location.hash = `#/backlog/${item.id}`)}>
                  Edit
                </button>
              )}
              {lane === "review" && (
                <>
                  {live && (
                    <button className="ghost" onClick={() => (location.hash = `#/s/${live.id}/review`)}>
                      Review
                    </button>
                  )}
                  <button className="ghost" disabled={busy} onClick={() => void status("shipped")}>
                    Ship
                  </button>
                </>
              )}
              {live ? (
                <button className="primary" onClick={() => (location.hash = `#/s/${live.id}`)}>
                  Open
                </button>
              ) : !item.project ? (
                <button className="primary" onClick={() => (location.hash = `#/backlog/${item.id}`)}>
                  Pick a folder
                </button>
              ) : (
                <span className="launch">
                  <select
                    aria-label="Agent"
                    value={agent}
                    onChange={(e) => setAgent(e.target.value as Agent)}
                    disabled={busy}
                  >
                    <option value="claude">Claude</option>
                    <option value="codex">Codex</option>
                  </select>
                  <button className="primary" disabled={busy} onClick={() => void launch()}>
                    {busy ? "Starting…" : lane === "review" ? "Relaunch" : "Launch"}
                  </button>
                </span>
              )}
            </>
          )}
        </div>
      </article>
    </div>
  );
}

interface Attachment {
  key: number;
  name: string;
  path: string | null;
}
let attachmentKey = 0;

/**
 * Writing an item down (`id` absent) or changing one. A new item is one box, the
 * first line its title, so a thought dictated on a phone goes in as it comes.
 */
export function ItemEditor({ id }: { id: number | null }) {
  const token = useApp((s) => s.token);
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const item = id === null ? null : (items.find((i) => i.id === id) ?? null);
  // An agent already on it: saving shouldn't start a second one.
  const running = item !== null && liveSession(item, sessions, boot) !== null;
  const [kind, setKind] = useState<ItemKind>(item?.kind ?? "bug");
  const [title, setTitle] = useState(item?.title ?? "");
  const [text, setText] = useState(() => item?.body ?? (id === null ? (read(DRAFT_KEY) ?? "") : ""));
  const [folder, setFolder] = useState<string>(() => item?.project ?? (id === null ? (read(FOLDER_KEY) ?? "") : ""));
  const [places, setPlaces] = useState<Places | null>(null);
  const [files, setFiles] = useState<Attachment[]>([]);
  const [busy, setBusy] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  // The item as the editor opened it, to send only what was changed here.
  const loaded = useRef<Item | null>(item);

  // An edit opened before the items arrived fills in once they do.
  useEffect(() => {
    if (!item || loaded.current) return;
    loaded.current = item;
    setKind(item.kind);
    setTitle(item.title);
    setText(item.body);
    setFolder(item.project ?? "");
  }, [item]);

  useEffect(() => {
    if (!token) return;
    getJson<Places>(token, "/api/places").then(setPlaces, (e: Error) => toast(`Can't list folders: ${e.message}`));
  }, [token]);

  useEffect(() => {
    if (id === null) write(DRAFT_KEY, text || null);
  }, [id, text]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(Math.max(el.scrollHeight, 120), 320)}px`;
  }, [text]);

  const attach = (picked: Iterable<File>) => {
    for (const file of picked) {
      const key = ++attachmentKey;
      setFiles((fs) => [...fs, { key, name: file.name || "image.png", path: null }]);
      sendFile(file).then(
        (path) => setFiles((fs) => fs.map((f) => (f.key === key ? { ...f, path } : f))),
        (e: Error) => {
          setFiles((fs) => fs.filter((f) => f.key !== key));
          toast(`${file.name}: ${e.message}`);
        },
      );
    }
  };

  const sending = files.some((f) => f.path === null);
  const ready = files.flatMap((f) => (f.path ? [f.path] : []));
  const empty = id === null ? !text.trim() : !title.trim();

  const save = async (andLaunch: boolean) => {
    if (busy || sending || empty) return;
    setBusy(true);
    try {
      let saved = id;
      if (id === null) {
        saved = await addItem({ title: "", body: text, kind, cwd: folder || null, attachments: ready });
        write(DRAFT_KEY, null);
        write(FOLDER_KEY, folder || null);
      } else {
        const change = editChange(loaded.current, item, { title, body: text, kind, project: folder || null });
        if (typeof change === "string") {
          // Seen now: saving again replaces the other change with this one.
          loaded.current = item;
          toast(change);
          return;
        }
        if (change && !(await changeItem(id, change))) return;
      }
      navigator.vibrate?.(10);
      if (andLaunch && saved !== null) {
        const agent = read(AGENT_KEY) === "codex" ? "codex" : "claude";
        const session = await launchItem(saved, agent);
        location.replace(`#/s/${session}`);
      } else {
        location.replace("#/backlog");
      }
    } catch (e) {
      toast((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  // Every repository it could be for, the one it's in first.
  const choices = new Map<string, string>();
  if (folder) choices.set(folder, base(folder));
  for (const p of [...(places?.recent ?? []), ...(places?.repos ?? [])]) {
    if (p.git && !choices.has(p.path)) choices.set(p.path, p.label);
  }

  return (
    <div
      className="page launcher-page"
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          void save(false);
        }
      }}
    >
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={back}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">{id === null ? "Write it down" : `Edit #${id}`}</div>
        </div>
      </header>

      <main className="launcher">
        <div className="kind-pick" role="radiogroup" aria-label="Kind">
          {ITEM_KINDS.map((k) => (
            <button
              key={k}
              role="radio"
              aria-checked={kind === k}
              className={`kind-chip kind-${k} ${kind === k ? "on" : ""}`}
              onClick={() => setKind(k)}
            >
              {k}
            </button>
          ))}
        </div>

        {id !== null && (
          <label className="field">
            <span className="field-label">Title</span>
            <input value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
        )}

        <label className="field">
          <span className="field-label">{id === null ? "What is it?" : "Details"}</span>
          <div
            className="field-row"
            onDragOver={(e) => {
              if (id === null && e.dataTransfer.types.includes("Files")) e.preventDefault();
            }}
            onDrop={(e) => {
              if (id !== null || !e.dataTransfer.files.length) return;
              e.preventDefault();
              attach(e.dataTransfer.files);
            }}
          >
            <textarea
              ref={area}
              rows={4}
              autoFocus={id === null}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onPaste={(e) => {
                if (id !== null || !e.clipboardData.files.length) return;
                e.preventDefault();
                attach(e.clipboardData.files);
              }}
              placeholder={
                kind === "bug"
                  ? "What broke, and how to make it happen. The first line is the title."
                  : "The idea, as rough as it is. The first line is the title."
              }
            />
            <MicButton value={text} onChange={setText} />
          </div>
        </label>

        {id === null && (
          <div className="item-attach">
            <input
              ref={picker}
              type="file"
              accept="image/*,*/*"
              multiple
              hidden
              onChange={(e) => {
                if (e.target.files) attach(e.target.files);
                e.target.value = "";
              }}
            />
            <button type="button" className="ghost" onClick={() => picker.current?.click()}>
              <Clip /> Screenshot or file
            </button>
            {files.map((f) => (
              <span key={f.key} className={`attachment ${f.path ? "" : "sending"}`}>
                {f.path === null && <span className="spin" />}
                <span className="attachment-name">{f.name}</span>
                <button type="button" aria-label={`Remove ${f.name}`} onClick={() => setFiles((fs) => fs.filter((x) => x.key !== f.key))}>
                  ×
                </button>
              </span>
            ))}
          </div>
        )}

        <label className="field">
          <span className="field-label">Repository</span>
          <select value={folder} onChange={(e) => setFolder(e.target.value)} disabled={!!item?.checkout}>
            <option value="">Not sure yet</option>
            {[...choices].map(([path, label]) => (
              <option key={path} value={path}>
                {label}
              </option>
            ))}
          </select>
        </label>
      </main>

      <footer className="dock item-dock">
        <button className="ghost" disabled={busy || sending || empty} onClick={() => void save(false)}>
          Save
        </button>
        <button
          className="primary start"
          disabled={busy || sending || empty || !folder || running || (item !== null && item.status !== "open")}
          onClick={() => void save(true)}
        >
          {busy ? "Saving…" : item?.sessions?.length ? "Save & relaunch" : "Save & launch"}
        </button>
      </footer>
    </div>
  );
}
