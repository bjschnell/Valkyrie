import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getJson, type Places } from "../lib/api";
import { COLUMNS, agentFiled, base, byColumn, cardActions, changesSomething, editChange, factsLine, inRepo, launchesOn, launchesShown, liveSession, noteOrigin, ordersShown, planScope, prLink, priority, repoChoices, startAgent, stepChanges, waitingOn, type Lane, type RepoFilter } from "../lib/backlog";
import { age } from "../lib/time";
import { ITEM_KINDS, PRIORITIES, type Item, type ItemFacts, type ItemKind, type LaunchOffer, type OrderProposal, type Priority } from "../proto";
import { addItem, answerLaunch, answerOrder, changeItem, checkMerged, deleteItem, itemFacts, launchItem, noteItem, planOrder, sendFile, shipMerged, toast, useApp } from "../store";
import { Back, Clip, Plus } from "./icons";
import { MicButton } from "./Mic";

type Agent = "claude" | "codex";
const AGENT_KEY = "valk.backlog.agent";
const FOLDER_KEY = "valk.backlog.folder";
const DRAFT_KEY = "valk.backlog.draft";
/** The repository the board shows, `""` for no folder; absent for every one. */
const REPO_KEY = "valk.backlog.repo";
/** The picker's value for every repository: no project path is relative. */
const ALL = "*";

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
  const all = useApp((s) => s.items);
  const orders = useApp((s) => s.orders);
  const launches = useApp((s) => s.launches);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const [showDone, setShowDone] = useState(false);
  const choices = repoChoices(all);
  const [picked, setPicked] = useState<RepoFilter>(() => read(REPO_KEY));
  // A repository whose items are all gone shows every one rather than an empty board.
  const repo = picked !== null && choices.some((c) => c.repo === picked) ? picked : null;
  const pick = (r: RepoFilter) => {
    setPicked(r);
    setReady(null);
    write(REPO_KEY, r);
  };
  const items = all.filter((i) => inRepo(i, repo));
  const columns = byColumn(items, sessions, boot, all);
  const open = items.length - columns.done.length;
  // Only items with a branch can be found merged.
  const mergeable = items.some((i) => (i.status === "open" || agentFiled(i)) && i.branch && i.project);
  const [shipping, setShipping] = useState(false);
  // Two taps, as delete asks: the first finds what's merged and lists it, the second
  // ships exactly those, if they still are.
  const [ready, setReady] = useState<number[] | null>(null);
  useEffect(() => {
    if (!ready) return;
    const t = setTimeout(() => setReady(null), 8000);
    return () => clearTimeout(t);
  }, [ready]);
  const listed = ready?.map((id) => `#${id}`).join(" ");
  const shipLabel = shipping ? (ready ? "Shipping…" : "Checking…") : ready ? `Ship ${listed}` : "Ship merged";
  const ship = async () => {
    setShipping(true);
    try {
      if (ready) {
        await shipMerged(ready);
        setReady(null);
      } else {
        const found = await checkMerged((id) => items.some((i) => i.id === id));
        if (found.length) setReady(found);
      }
    } finally {
      setShipping(false);
    }
  };

  // A build order (ADR-0019 §2): asked of a model, so it can take a minute.
  const scope = planScope(repo, items);
  const [planning, setPlanning] = useState(false);
  const plan = async () => {
    if (!scope) return;
    setPlanning(true);
    try {
      await planOrder(scope);
    } finally {
      setPlanning(false);
    }
  };
  const shownOrders = ordersShown(orders, repo);
  const shownLaunches = launchesShown(launches, repo);

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
        {open > 1 && (
          <button
            className="ghost"
            disabled={planning || !scope}
            title={scope ? "A model proposes what to do first; nothing changes until you accept" : "Pick a repository first"}
            onClick={() => void plan()}
          >
            {planning ? "Planning…" : "Plan order"}
          </button>
        )}
        {mergeable && (
          <button
            className={ready ? "primary" : "ghost"}
            disabled={shipping}
            title="As of the last fetch; valk todo ship --merged fetches first"
            onClick={() => void ship()}
          >
            {shipLabel}
          </button>
        )}
        <button className="icon-btn" aria-label="Write something down" onClick={() => (location.hash = "#/backlog/new")}>
          <Plus />
        </button>
      </header>
      <main className="inbox backlog">
        {(choices.length > 1 || repo !== null) && (
          <div className="backlog-filter">
            <select
              aria-label="Repository"
              value={repo ?? ALL}
              onChange={(e) => pick(e.target.value === ALL ? null : e.target.value)}
            >
              <option value={ALL}>Every repository</option>
              {choices.map((c) => (
                <option key={c.repo} value={c.repo} title={c.repo || undefined}>
                  {c.label}
                </option>
              ))}
            </select>
          </div>
        )}
        {shownLaunches.map((o) => (
          <LaunchCard key={`${o.project}:${o.id}`} offer={o} />
        ))}
        {shownOrders.map((o) => (
          <OrderCard key={`${o.project}:${o.id}`} order={o} items={all} />
        ))}
        {open === 0 && repo !== null && (
          <div className="calm backlog-calm">
            <div className="calm-title">Nothing open here</div>
            <button className="ghost" onClick={() => pick(null)}>
              Show every repository
            </button>
          </div>
        )}
        {open === 0 && repo === null && (
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

/**
 * A ready item the supervisor offers to start (ADR-0019 §2): Start launches it as
 * the card's Launch would, on its last agent and model; Leave offers the next one.
 */
function LaunchCard({ offer }: { offer: LaunchOffer }) {
  const [busy, setBusy] = useState(false);
  const answer = async (launch: boolean) => {
    setBusy(true);
    try {
      const id = await answerLaunch(offer, launch);
      if (id !== null) {
        navigator.vibrate?.(10);
        location.hash = `#/s/${id}`;
      }
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="order-card" aria-label={`Ready to start: #${offer.item}`}>
      <div className="order-head">
        <div className="order-title">
          Ready to start in {base(offer.project)}: <span className="order-id">#{offer.item}</span> {offer.title}
        </div>
        <div className="order-why">
          {offer.why} · on {launchesOn(offer)}
        </div>
      </div>
      <div className="order-actions">
        <button className="ghost" disabled={busy} onClick={() => void answer(false)}>
          Leave
        </button>
        <button className="primary" disabled={busy} onClick={() => void answer(true)}>
          Start
        </button>
      </div>
    </section>
  );
}

/**
 * A build order waiting on the user: each item in the order proposed, why, and what
 * accepting it changes. Untick an item to leave it as it is; nothing changes until
 * Accept, and Decline changes nothing.
 */
function OrderCard({ order, items }: { order: OrderProposal; items: Item[] }) {
  const [skip, setSkip] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const changing = order.steps.filter((s) => changesSomething(s));
  const ticked = changing.filter((s) => !skip.includes(s.item)).map((s) => s.item);
  const toggle = (id: number) => setSkip((k) => (k.includes(id) ? k.filter((i) => i !== id) : [...k, id]));
  const answer = async (accept: boolean) => {
    setBusy(true);
    try {
      await answerOrder(order, accept, accept && skip.length ? ticked : undefined);
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="order-card" aria-label={`Build order for ${base(order.project)}`}>
      <div className="order-head">
        <div className="order-title">Build order for {base(order.project)}</div>
        <div className="order-why">{order.trigger}</div>
      </div>
      <ol className="order-steps">
        {order.steps.map((step) => {
          const item = items.find((i) => i.id === step.item);
          const changes = stepChanges(step, item);
          const can = changesSomething(step);
          return (
            <li key={step.item} className={can && skip.includes(step.item) ? "order-skipped" : ""}>
              <label className="order-step">
                {can ? (
                  <input type="checkbox" checked={!skip.includes(step.item)} disabled={busy} onChange={() => toggle(step.item)} />
                ) : (
                  <span className="order-nobox" />
                )}
                <span className="order-id">#{step.item}</span>
                <span className="order-name">{item?.title ?? "(gone)"}</span>
                {changes && <span className="order-change">{changes}</span>}
              </label>
              {step.reason && <div className="order-reason">{step.reason}</div>}
            </li>
          );
        })}
      </ol>
      <div className="order-actions">
        <button className="ghost" disabled={busy} onClick={() => void answer(false)}>
          Decline
        </button>
        <button className="primary" disabled={busy || (changing.length > 0 && ticked.length === 0)} onClick={() => void answer(true)}>
          {changing.length === 0 ? "Clear" : ticked.length === changing.length ? "Accept all" : `Accept ${ticked.length}`}
        </button>
      </div>
    </section>
  );
}

function ItemCard({ item, lane }: { item: Item; lane: Lane }) {
  const now = useApp((s) => s.now);
  const items = useApp((s) => s.items);
  const waits = lane === "done" ? [] : waitingOn(item, items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [agent, setAgent] = useState<Agent>(() => startAgent(item, read(AGENT_KEY)));
  const live = liveSession(item, sessions, boot);
  // Accept, Open, Launch: Launch beside Open while only takes are live (it hands
  // the item over), Accept beside it for an agent's own filed work.
  const actions = cardActions(item, sessions, boot);
  const folder = item.project ? base(item.project) : null;
  const pr = prLink(item.pr);

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
  // Back to the backlog stops the agent on it, so with one running it asks twice too.
  const [back, setBack] = useState(false);
  useEffect(() => {
    if (!back) return;
    const t = setTimeout(() => setBack(false), 5000);
    return () => clearTimeout(t);
  }, [back]);
  const startedBefore = (item.sessions?.length ?? 0) > 0 && !item.pr;
  const sendBack = () => (live && !back ? setBack(true) : act(() => changeItem(item.id, { do: "return" })));

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
          {lane !== "done" && <span className={`kind-chip prio-${priority(item)}`}>{priority(item)}</span>}
          <span className="card-prog">#{item.id}</span>
          <span className="card-prog item-folder">{folder ?? "no folder"}</span>
          <span className="card-age">{lane === "done" ? item.status : age(item.updated * 1000, now)}</span>
        </button>
        <p className="decision-title item-title" onClick={() => setExpanded(!expanded)}>
          {item.title}
        </p>
        {waits.length > 0 && (
          <div className="item-live item-after">
            after {waits.map((id) => `#${id}`).join(", ")}, not finished yet
          </div>
        )}
        {lane === "proposed" && (
          <div className="item-live">
            <strong className="item-needs item-proposed-pill">Proposed</strong>by {item.by}, waiting on you
          </div>
        )}
        {agentFiled(item) && (
          <div className="item-live">
            <strong className="item-needs item-proposed-pill">Agent-filed</strong>by {item.by}, not accepted yet
          </div>
        )}
        {live && (
          <div className="item-live">
            {lane === "needs" && <strong className="item-needs">Needs you</strong>}
            {live.name} · {live.status.state.replace("_", " ")}
          </div>
        )}
        {lane === "review" && pr && (
          <div className="item-live">{item.pr_closed ? "PR closed without merging" : "PR open"}</div>
        )}
        {!expanded && lane === "review" && latest && (
          <p className="item-handoff" onClick={() => setExpanded(true)}>
            <span className="item-handoff-by">
              {who(latest.by)}
              {noteOrigin(latest)}:
            </span>{" "}
            {latest.text}
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
              {pr && (
                <>
                  {" · "}
                  <a href={pr} target="_blank" rel="noopener noreferrer">
                    pull request
                  </a>
                </>
              )}
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
                  {noteOrigin(n)}
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
              {expanded && startedBefore && (
                <button className={`ghost ${back ? "item-sure" : ""}`} disabled={busy} onClick={() => void sendBack()}>
                  {back ? "Stop it and return" : "Back to backlog"}
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
              {actions.map((a, i) => {
                const kind = i === actions.length - 1 ? "primary" : "ghost";
                if (a === "accept")
                  return (
                    <button key={a} className={kind} disabled={busy} onClick={() => void status("open")}>
                      Accept
                    </button>
                  );
                if (a === "open")
                  return (
                    <button key={a} className={kind} onClick={() => (location.hash = `#/s/${live?.id}`)}>
                      Open
                    </button>
                  );
                if (a === "folder")
                  return (
                    <button key={a} className={kind} onClick={() => (location.hash = `#/backlog/${item.id}`)}>
                      Pick a folder
                    </button>
                  );
                return (
                  <span key={a} className="launch">
                    <select
                      aria-label="Agent"
                      value={agent}
                      onChange={(e) => setAgent(e.target.value as Agent)}
                      disabled={busy}
                    >
                      <option value="claude">Claude</option>
                      <option value="codex">Codex</option>
                    </select>
                    <button className={kind} disabled={busy} onClick={() => void launch()}>
                      {busy ? "Starting…" : lane === "review" ? "Relaunch" : "Launch"}
                    </button>
                  </span>
                );
              })}
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
const idList = (ids: number[] | undefined) => (ids ?? []).map((i) => `#${i}`).join(", ");

/** "#12, 14" as [12, 14]; null if something in it isn't an item number. */
function parseIds(text: string): number[] | null {
  const parts = text.split(/[\s,]+/).filter(Boolean);
  const ids = parts.map((p) => (/^#?\d+$/.test(p) ? Number(p.replace("#", "")) : NaN));
  return ids.some((n) => !Number.isInteger(n) || n <= 0) ? null : ids;
}

export function ItemEditor({ id }: { id: number | null }) {
  const token = useApp((s) => s.token);
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const item = id === null ? null : (items.find((i) => i.id === id) ?? null);
  // An agent already on it: saving shouldn't start a second one.
  const running = item !== null && liveSession(item, sessions, boot) !== null;
  const [kind, setKind] = useState<ItemKind>(item?.kind ?? "bug");
  const [prio, setPrio] = useState<Priority>(item ? priority(item) : "p2");
  // What it comes after, as typed: "#12, 14".
  const [after, setAfter] = useState(() => idList(item?.blocked_by));
  const [title, setTitle] = useState(item?.title ?? "");
  const [text, setText] = useState(() => item?.body ?? (id === null ? (read(DRAFT_KEY) ?? "") : ""));
  const [folder, setFolder] = useState<string>(
    // A new item goes where the board is filtered to, else where the last one went.
    () => item?.project ?? (id === null ? read(REPO_KEY) || (read(FOLDER_KEY) ?? "") : ""),
  );
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
    setPrio(priority(item));
    setAfter(idList(item.blocked_by));
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
    const blocked_by = parseIds(after);
    if (!blocked_by) return toast("Comes after: item numbers, like #12, 14");
    setBusy(true);
    try {
      let saved = id;
      if (id === null) {
        saved = await addItem({ title: "", body: text, kind, cwd: folder || null, attachments: ready, priority: prio, blocked_by });
        write(DRAFT_KEY, null);
        write(FOLDER_KEY, folder || null);
      } else {
        const change = editChange(loaded.current, item, {
          title,
          body: text,
          kind,
          project: folder || null,
          priority: prio,
          blocked_by,
        });
        if (change && "clash" in change) {
          // Seen now: saving again replaces the other change with this one.
          loaded.current = change.was;
          toast(change.clash);
          return;
        }
        if (change && !(await changeItem(id, change))) return;
      }
      navigator.vibrate?.(10);
      if (andLaunch && saved !== null) {
        const session = await launchItem(saved, startAgent(item, read(AGENT_KEY)));
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

        <div className="kind-pick" role="radiogroup" aria-label="Priority">
          {PRIORITIES.map((p) => (
            <button
              key={p}
              role="radio"
              aria-checked={prio === p}
              className={`kind-chip prio-${p} ${prio === p ? "on" : ""}`}
              onClick={() => setPrio(p)}
            >
              {p}
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
          <span className="field-label">Comes after</span>
          <input
            value={after}
            inputMode="numeric"
            placeholder="Items to finish first, like #12, 14"
            onChange={(e) => setAfter(e.target.value)}
          />
        </label>

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
        <button className="primary save" disabled={busy || sending || empty} onClick={() => void save(false)}>
          Save
        </button>
        <button
          className="ghost launch"
          disabled={busy || sending || empty || !folder || running || (item !== null && item.status !== "open")}
          onClick={() => void save(true)}
        >
          {busy ? "Saving…" : item?.sessions?.length ? "Save & relaunch" : "Save & launch"}
        </button>
      </footer>
    </div>
  );
}
