import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { editPatch } from "../lib/optimistic";
import { COLUMNS, agentFiled, base, byColumn, cardActions, changesSomething, editChange, factsLine, inRepo, kindCounts, kindFilter, launchModel, launchesOn, itemNo, launchesShown, liveSession, MODEL_CHOICES, noteOrigin, orderHref, orderSize, ofKind, ordersShown, pieces, planScope, prLink, priority, progress, repoChoices, startAgent, startModel, stepChanges, waitingOn, type KindFilter, type Lane, type RepoFilter } from "../lib/backlog";
import { age } from "../lib/time";
import { ITEM_KINDS, PRIORITIES, type Item, type ItemFacts, type ItemKind, type LaunchOffer, type OrderProposal, type Priority } from "../proto";
import { addItem, answerLaunch, answerOrder, changeItem, checkMerged, editItem, itemFacts, takeFailedEdit, launchItem, noteItem, planOrder, removeItem, sendFile, setStatus, shipMerged, toast, useApp } from "../store";
import { RepoSelect } from "./Repos";
import { Back, Clip, Plus } from "./icons";
import { MicButton } from "./Mic";
import { usePageKeys } from "../lib/pageKeys";

type Agent = "claude" | "codex";
const AGENT_KEY = "valk.backlog.agent";
const FOLDER_KEY = "valk.backlog.folder";
const DRAFT_KEY = "valk.backlog.draft";
/** The repository the board shows, `""` for no folder; absent for every one. */
const REPO_KEY = "valk.backlog.repo";
/** The one kind of item the board shows; absent for every kind. A new item
 * starts as that kind too. */
const KIND_KEY = "valk.backlog.kind";
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
  const [kind, setKind] = useState<KindFilter>(() => kindFilter(read(KIND_KEY)));
  const pickKind = (k: KindFilter) => {
    setKind(k);
    setReady(null);
    write(KIND_KEY, k);
  };
  // A proposal's toast opens its card (#194): one a filter hides shows every
  // repository, or every kind, instead, without keeping that choice.
  const focus = useApp((s) => s.focusItem);
  const focused = focus === null ? undefined : all.find((i) => i.id === focus);
  useEffect(() => {
    if (focus === null) return;
    if (!focused) useApp.setState({ focusItem: null });
    else {
      if (!inRepo(focused, repo)) setPicked(null);
      if (!ofKind(focused, kind)) setKind(null);
    }
  }, [focus, focused, repo, kind]);
  const inThisRepo = all.filter((i) => inRepo(i, repo));
  const counts = kindCounts(inThisRepo);
  const items = inThisRepo.filter((i) => ofKind(i, kind));
  const columns = byColumn(items, sessions, boot, all);
  const open = items.length - columns.done.length;
  // A build order covers a repository's every open item, whatever kind is shown.
  const repoOpen = inThisRepo.filter((i) => i.status === "open" || i.status === "proposed").length;
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
  const listed = ready?.map((id) => `#${itemNo(all, id)}`).join(" ");
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
  const scope = planScope(repo, inThisRepo);
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

  const newItem = () => (location.hash = "#/backlog/new");
  usePageKeys("backlog", { n: newItem, a: newItem, q: () => (location.hash = "#/") });

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => (location.hash = "#/")}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Backlog</h1>
        </div>
        {repoOpen > 1 && (
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
        {(choices.length > 1 || repo !== null || inThisRepo.length > 0) && (
          <div className="backlog-filter">
            {(choices.length > 1 || repo !== null) && (
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
            )}
            <div className="kind-pick" role="radiogroup" aria-label="Kind">
              <button role="radio" aria-checked={kind === null} className={`kind-chip ${kind === null ? "on" : ""}`} onClick={() => pickKind(null)}>
                all
              </button>
              {ITEM_KINDS.map((k) => (
                <button
                  key={k}
                  role="radio"
                  aria-checked={kind === k}
                  className={`kind-chip kind-${k} ${kind === k ? "on" : ""}`}
                  // Tapping the one shown again shows every kind.
                  onClick={() => pickKind(kind === k ? null : k)}
                >
                  {k} <span className="count">{counts[k]}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        {shownLaunches.map((o) => (
          <LaunchCard key={`${o.project}:${o.id}`} offer={o} />
        ))}
        {shownOrders.map((o) => (
          <OrderBanner key={`${o.project}:${o.id}`} order={o} />
        ))}
        {open === 0 && kind !== null && (
          <div className="calm backlog-calm">
            <div className="calm-title">
              No open {kind} items{repo !== null ? " here" : ""}
            </div>
            <button className="ghost" onClick={() => pickKind(null)}>
              Show every kind
            </button>
          </div>
        )}
        {open === 0 && kind === null && repo !== null && (
          <div className="calm backlog-calm">
            <div className="calm-title">Nothing open here</div>
            <button className="ghost" onClick={() => pick(null)}>
              Show every repository
            </button>
          </div>
        )}
        {open === 0 && kind === null && repo === null && (
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
    <section className="order-card" aria-label={`Ready to start: #${offer.number}`}>
      <div className="order-head">
        <div className="order-title">
          Ready to start in {base(offer.project)}: <span className="order-id">#{offer.number}</span> {offer.title}
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
 * A build order waiting on the user, as one line above the board: a big backlog's
 * order is a page of its own (`OrderPage`), not a wall over the cards.
 */
function OrderBanner({ order }: { order: OrderProposal }) {
  return (
    <section className="order-card order-banner" aria-label={`Build order for ${base(order.project)}`}>
      <div className="order-head">
        <div className="order-title">Build order for {base(order.project)}</div>
        <div className="order-why">
          {orderSize(order)} · {order.trigger}
        </div>
      </div>
      <button className="primary" onClick={() => (location.hash = orderHref(order.project))}>
        Review
      </button>
    </section>
  );
}

/** A repository's build order on a page of its own, or a note that none waits. */
export function OrderPage({ project }: { project: string }) {
  const orders = useApp((s) => s.orders);
  const items = useApp((s) => s.items);
  const order = orders.find((o) => o.project === project);
  const back = () => (location.hash = "#/backlog");

  usePageKeys("order", { q: back, Escape: back });

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={back}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Build order for {base(project)}</h1>
          {order && (
            <div className="title-sub">
              {orderSize(order)} · {order.trigger}
            </div>
          )}
        </div>
      </header>
      <main className="inbox backlog order-page">
        {order ? (
          // A replaced proposal starts over: its ticks were for other steps.
          <OrderReview key={order.id} order={order} items={items} onDone={back} />
        ) : (
          <div className="calm backlog-calm">
            <div className="calm-title">No build order waits here</div>
            <div className="calm-sub">It was answered, or replaced by a newer one. Plan order on the backlog asks for one.</div>
            <button className="ghost" onClick={back}>
              Back to the backlog
            </button>
          </div>
        )}
      </main>
    </div>
  );
}

/**
 * A build order's steps: each item in the order proposed, why, and what accepting
 * it changes. Untick an item to leave it as it is; nothing changes until Accept,
 * and Decline changes nothing (decision #47).
 */
function OrderReview({ order, items, onDone }: { order: OrderProposal; items: Item[]; onDone: () => void }) {
  const [skip, setSkip] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const changing = order.steps.filter((s) => changesSomething(s));
  const ticked = changing.filter((s) => !skip.includes(s.item)).map((s) => s.item);
  const toggle = (id: number) => setSkip((k) => (k.includes(id) ? k.filter((i) => i !== id) : [...k, id]));
  const all = ticked.length === changing.length;
  const answer = async (accept: boolean) => {
    setBusy(true);
    try {
      if (await answerOrder(order, accept, accept && skip.length ? ticked : undefined)) onDone();
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <ol className="order-steps">
        {order.steps.map((step, n) => {
          const item = items.find((i) => i.id === step.item);
          const changes = stepChanges(step, item, items);
          const can = changesSomething(step);
          return (
            <li key={step.item} className={can && skip.includes(step.item) ? "order-skipped" : ""}>
              <label className="order-step">
                {can ? (
                  <input type="checkbox" checked={!skip.includes(step.item)} disabled={busy} onChange={() => toggle(step.item)} />
                ) : (
                  <span className="order-nobox" />
                )}
                <span className="order-n">{n + 1}.</span>
                <span className="order-id">#{item?.number ?? step.item}</span>
                <span className="order-name">{item?.title ?? "(gone)"}</span>
                {changes && <span className="order-change">{changes}</span>}
              </label>
              {step.reason && <div className="order-reason">{step.reason}</div>}
            </li>
          );
        })}
      </ol>
      <div className="order-actions order-bar">
        {changing.length > 1 && (
          <button className="ghost order-all" disabled={busy} onClick={() => setSkip(all ? changing.map((s) => s.item) : [])}>
            {all ? "Untick all" : "Tick all"}
          </button>
        )}
        <button className="ghost" disabled={busy} onClick={() => void answer(false)}>
          Decline
        </button>
        <button className="primary" disabled={busy || (changing.length > 0 && ticked.length === 0)} onClick={() => void answer(true)}>
          {changing.length === 0 ? "Clear" : all ? "Accept all" : `Accept ${ticked.length}`}
        </button>
      </div>
    </>
  );
}

function ItemCard({ item, lane }: { item: Item; lane: Lane }) {
  const now = useApp((s) => s.now);
  const items = useApp((s) => s.items);
  const waits = lane === "done" ? [] : waitingOn(item, items);
  // Its group: the item it's a piece of, or how far its own pieces are.
  const mine = pieces(item, items);
  const [done, of] = progress(item, items);
  const up = item.parent === undefined ? undefined : items.find((i) => i.id === item.parent);
  const group = up ? `piece of #${up.number} ${up.title}` : of > 0 ? `${done}/${of} pieces done` : null;
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const [expanded, setExpanded] = useState(false);
  // Opened from a proposal's toast: this card, open and in view, once.
  const focused = useApp((s) => s.focusItem === item.id);
  const cardRef = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    if (!focused) return;
    setExpanded(true);
    cardRef.current?.scrollIntoView({ block: "center" });
    useApp.setState({ focusItem: null });
  }, [focused]);
  const [busy, setBusy] = useState(false);
  const [agent, setAgent] = useState<Agent>(() => startAgent(item, read(AGENT_KEY)));
  // Edited text, or null while the field follows the agent: its last model on the item.
  const [typed, setTyped] = useState<string | null>(null);
  const model = typed ?? startModel(item, agent);
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
        const id = await launchItem(item.id, agent, true, typed === null ? undefined : launchModel(typed) ?? "");
        navigator.vibrate?.(10);
        location.hash = `#/s/${id}`;
      } catch (e) {
        toast(`Couldn't start: ${(e as Error).message}`);
      }
    });
  // Shown at once with Undo in the toast; the daemon hears when the Undo is gone.
  const status = (s: Item["status"]) => setStatus(item.id, s);
  const remove = () => removeItem(item.id);
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
      <article ref={cardRef} className={`card item-card ${EDGE[lane]} ${lane === "done" ? "item-done" : ""}`}>
        <button className="card-head" onClick={() => setExpanded(!expanded)} aria-expanded={expanded}>
          <span className={`kind-chip kind-${item.kind}`}>{item.kind}</span>
          {lane !== "done" && <span className={`kind-chip prio-${priority(item)}`}>{priority(item)}</span>}
          <span className="card-prog">#{item.number}</span>
          <span className="card-prog item-folder">{folder ?? "no folder"}</span>
          <span className="card-age">{lane === "done" ? item.status : age(item.updated * 1000, now)}</span>
        </button>
        <p className="decision-title item-title" onClick={() => setExpanded(!expanded)}>
          {item.title}
        </p>
        {waits.length > 0 && (
          <div className="item-live item-after">
            after {waits.map((id) => `#${itemNo(items, id)}`).join(", ")}, not finished yet
          </div>
        )}
        {group && <div className="item-live item-group">{group}</div>}
        {expanded && mine.length > 0 && (
          <ul className="item-pieces">
            {mine.map((p) => (
              <li key={p.id} className={p.status === "shipped" || p.status === "dropped" ? "item-piece-done" : undefined}>
                #{p.number} {p.title} <span className="card-age">{p.status}</span>
              </li>
            ))}
          </ul>
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
            <button className="ghost no" disabled={busy} onClick={remove}>
              Delete
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
                      onChange={(e) => {
                        setAgent(e.target.value as Agent);
                        setTyped(null);
                      }}
                      disabled={busy}
                    >
                      <option value="claude">Claude</option>
                      <option value="codex">Codex</option>
                    </select>
                    <input
                      aria-label="Model"
                      list={`models-${item.id}`}
                      value={model}
                      onChange={(e) => setTyped(e.target.value)}
                      placeholder="default"
                      disabled={busy}
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                    />
                    <datalist id={`models-${item.id}`}>
                      {MODEL_CHOICES[agent].map((m) => (
                        <option key={m} value={m} />
                      ))}
                    </datalist>
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
const idList = (ids: number[] | undefined, items: Item[]) => (ids ?? []).map((i) => `#${itemNo(items, i)}`).join(", ");

/** "#12, 14" as the ids of items 12 and 14 of the repository `folder` (empty for none);
 * null if something in it isn't an item there. */
function parseIds(text: string, items: Item[], folder: string): number[] | null {
  const parts = text.split(/[\s,]+/).filter(Boolean);
  const ids = parts.map((p) =>
    /^#?\d+$/.test(p) ? items.find((i) => (i.project ?? "") === folder && i.number === Number(p.replace("#", "")))?.id : undefined,
  );
  return ids.some((n) => n === undefined) ? null : (ids as number[]);
}

export function ItemEditor({ id }: { id: number | null }) {
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const item = id === null ? null : (items.find((i) => i.id === id) ?? null);
  // Back from a refused save: the editor opens with that edit in it.
  const [retry] = useState(() => (id === null ? null : takeFailedEdit(id)));
  const start = item && retry ? { ...item, ...editPatch(retry) } : item;
  // An agent already on it: saving shouldn't start a second one.
  const running = item !== null && liveSession(item, sessions, boot) !== null;
  // A new one starts as the kind the board shows.
  const [kind, setKind] = useState<ItemKind>(start?.kind ?? kindFilter(read(KIND_KEY)) ?? "bug");
  const [prio, setPrio] = useState<Priority>(start ? priority(start) : "p2");
  // What it comes after, as typed: "#12, 14".
  const [after, setAfter] = useState(() => idList(start?.blocked_by, items));
  // The item it's a piece of, as typed: "#7", or empty for none.
  const [partOf, setPartOf] = useState(() => idList(start?.parent ? [start.parent] : [], items));
  const [title, setTitle] = useState(start?.title ?? "");
  const [text, setText] = useState(() => start?.body ?? (id === null ? (read(DRAFT_KEY) ?? "") : ""));
  const [folder, setFolder] = useState<string>(
    // A new item goes where the board is filtered to, else where the last one went.
    () => start?.project ?? (id === null ? read(REPO_KEY) || (read(FOLDER_KEY) ?? "") : ""),
  );
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
    setAfter(idList(item.blocked_by, items));
    setPartOf(idList(item.parent ? [item.parent] : [], items));
    setTitle(item.title);
    setText(item.body);
    setFolder(item.project ?? "");
  }, [item]);

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
    const blocked_by = parseIds(after, items, folder);
    if (!blocked_by) return toast("Comes after: item numbers in this repository, like #12, 14");
    const up = parseIds(partOf, items, folder);
    if (!up || up.length > 1) return toast("Piece of: one item number in this repository, like #7");
    const parent = up[0] ?? 0;
    setBusy(true);
    try {
      let saved = id;
      if (id === null) {
        saved = await addItem({ title: "", body: text, kind, cwd: folder || null, attachments: ready, priority: prio, blocked_by, parent: parent || undefined });
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
          parent,
        });
        if (change && "clash" in change) {
          // Seen now: saving again replaces the other change with this one.
          loaded.current = change.was;
          toast(change.clash);
          return;
        }
        if (change && andLaunch && !(await changeItem(id, change))) return;
        // Saved alone, it shows at once with Undo, and the daemon hears when that's gone.
        if (change && !andLaunch && change.do === "edit") editItem(id, change);
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
          <h1 className="title-name">{id === null ? "Write it down" : `Edit #${item?.number ?? id}`}</h1>
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
          <span className="field-label">Piece of</span>
          <input
            value={partOf}
            inputMode="numeric"
            placeholder="The item it's part of, like #7"
            onChange={(e) => setPartOf(e.target.value)}
          />
        </label>

        <label className="field">
          <span className="field-label">Repository</span>
          <RepoSelect value={folder} onChange={setFolder} none="Not sure yet" disabled={!!item?.checkout} />
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
