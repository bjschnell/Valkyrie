import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { editPatch } from "../lib/optimistic";
import { GROUPS, agentFiled, base, boardStep, byGroup, firstLane, cardActions, itemRef, rowState, tree, editChange, factsLine, inRepo, kindCounts, kindFilter, launchModel, itemNo, liveSession, MODEL_CHOICES, noteOrigin, ofKind, pieces, prLink, priority, progress, repoChoices, startAgent, startModel, type Group, type KindFilter, type Lane, type RepoFilter } from "../lib/backlog";
import { age } from "../lib/time";
import { ITEM_KINDS, PRIORITIES, type Item, type ItemFacts, type ItemKind, type Priority } from "../proto";
import { addItem, setStatuses, changeItem, checkMerged, editItem, itemFacts, takeFailedEdit, launchItem, noteItem, removeItem, sendFile, setStatus, shipMerged, toast, useApp } from "../store";
import { RepoSelect } from "./Repos";
import { Back, Chevron, Clip, KindIcon } from "./icons";
import { useDesktop, useMedia } from "./Terminal";
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

/** Room for the four lanes side by side; narrower, one lane at a time (#266). */
const LANES_SIDE = "(min-width: 900px)";
/** How many of Done the board shows until asked for more, and at most. */
const DONE_FEW = 5;
const DONE_MOST = 30;

const back = () => (history.length > 1 ? history.back() : (location.hash = "#/backlog"));

/** The edge color of a card, as the inbox's cards show a session's state. */
const EDGE: Record<Lane, string> = {
  proposed: "",
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
export function Backlog({ beside = false }: { beside?: boolean } = {}) {
  const all = useApp((s) => s.items);
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
  const { filed, groups } = byGroup(items, sessions, boot, all);
  const open = items.length - groups.done.length;
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

  // The board's lanes (#266), each a tree of rows; Done shows its newest few.
  const side = useMedia(LANES_SIDE);
  const laneRows = GROUPS.map(({ group }) => {
    const rows = tree(groups[group]);
    return group === "done" ? rows.slice(0, showDone ? DONE_MOST : DONE_FEW) : rows;
  });
  // j/k move down and up a lane, h/l between lanes, Enter opens or closes a
  // card, Esc closes it. Lanes follow sessions: nothing moves by hand (#15).
  const [cur, setCur] = useState<[number, number] | null>(null);
  const [opened, setOpened] = useState<number | null>(null);
  const selected = cur ? (laneRows[cur[0]]?.[cur[1]]?.item.id ?? null) : null;
  // A phone shows one lane: the one picked, else In progress or the first with cards.
  const [shown, setShown] = useState<Group | null>(null);
  const laneAt = GROUPS.findIndex(({ group }) => group === (shown ?? firstLane(groups)));
  const swipe = useRef<{ x: number; y: number } | null>(null);
  const showLane = (i: number) => {
    setShown(GROUPS[Math.max(0, Math.min(GROUPS.length - 1, i))].group);
    setCur(null);
  };
  const move = (key: "j" | "k" | "h" | "l") => {
    // On a phone the cursor stays in the lane on show.
    const sizes = laneRows.map((rows, i) => (side || i === laneAt ? rows.length : 0));
    const next = boardStep(sizes, cur, key);
    setCur(next);
    if (!next) return;
    const id = laneRows[next[0]][next[1]].item.id;
    document.querySelector<HTMLElement>(`.item-slot[data-item="${id}"]`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  };
  const toggle = (id: number) => setOpened((was) => (was === id ? null : id));
  const newItem = () => (location.hash = "#/backlog/new");
  usePageKeys("backlog", {
    n: newItem,
    a: newItem,
    q: () => (location.hash = "#/"),
    j: () => move("j"),
    k: () => move("k"),
    h: () => move("h"),
    l: () => move("l"),
    Enter: () => selected !== null && toggle(selected),
    Escape: () => {
      setOpened(null);
      setCur(null);
    },
  }, { off: beside });

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={() => (location.hash = "#/")}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Backlog</h1>
        </div>
        {mergeable && (
          <button
            className={`ghost ${ready ? "item-sure" : ""}`}
            disabled={shipping}
            title="As of the last fetch; valk todo ship --merged fetches first"
            onClick={() => void ship()}
          >
            {shipLabel}
          </button>
        )}
        {/* Beside the editor, its Save is the one primary. */}
        <button className={beside ? "ghost" : "primary"} title="Write something down (n)" onClick={newItem}>
          New item
        </button>
      </header>
      <main className={`inbox backlog ${side ? "backlog-lanes" : ""}`}>
        {(choices.length > 1 || repo !== null || inThisRepo.length > 0) && (
          <div className="backlog-filter">
            {(choices.length > 1 || repo !== null) && (
              <div className="seg" role="radiogroup" aria-label="Repository">
                <button role="radio" aria-checked={repo === null} className={`seg-item ${repo === null ? "on" : ""}`} onClick={() => pick(null)}>
                  All
                </button>
                {choices.map((c) => (
                  <button
                    key={c.repo}
                    role="radio"
                    aria-checked={repo === c.repo}
                    className={`seg-item ${repo === c.repo ? "on" : ""}`}
                    title={c.repo || undefined}
                    onClick={() => pick(c.repo)}
                  >
                    {c.label}
                  </button>
                ))}
              </div>
            )}
            <select aria-label="Kind" value={kind ?? ALL} onChange={(e) => pickKind(e.target.value === ALL ? null : (e.target.value as ItemKind))}>
              <option value={ALL}>Every kind</option>
              {ITEM_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k} ({counts[k]})
                </option>
              ))}
            </select>
          </div>
        )}
        {filed.length > 0 && <FiledTray items={filed} />}
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
          </div>
        )}
        {items.length > 0 && !side && (
          <div className="seg lane-switch" role="tablist" aria-label="Lane">
            {GROUPS.map(({ group, short }, i) => (
              <button
                key={group}
                role="tab"
                aria-selected={i === laneAt}
                className={`seg-item ${i === laneAt ? "on" : ""}`}
                onClick={() => showLane(i)}
              >
                {short}
                {group !== "done" && <span className="count">{groups[group].length}</span>}
              </button>
            ))}
          </div>
        )}
        {items.length > 0 && (
          <div
            className="lanes"
            onTouchStart={(e) => (swipe.current = { x: e.touches[0].clientX, y: e.touches[0].clientY })}
            onTouchEnd={(e) => {
              const from = swipe.current;
              swipe.current = null;
              if (side || !from) return;
              const dx = e.changedTouches[0].clientX - from.x;
              const dy = e.changedTouches[0].clientY - from.y;
              if (Math.abs(dx) > 60 && Math.abs(dx) > 2 * Math.abs(dy)) showLane(laneAt + (dx < 0 ? 1 : -1));
            }}
          >
            {GROUPS.map(({ group, title }, li) => {
              if (!side && li !== laneAt) return null;
              const list = groups[group];
              const more = Math.min(list.length, DONE_MOST) - DONE_FEW;
              return (
                <section key={group} className={`lane lane-${group}`} aria-label={title}>
                  {side && (
                    <h2 className="section-title lane-title">
                      {title} <span className="count">{list.length}</span>
                    </h2>
                  )}
                  <div className="list lane-list">
                    {laneRows[li].map(({ item, lane, depth }, ri) => (
                      <ItemCard
                        key={item.id}
                        item={item}
                        lane={lane}
                        depth={depth}
                        picked={cur !== null && cur[0] === li && cur[1] === ri}
                        open={opened === item.id}
                        onToggle={() => toggle(item.id)}
                      />
                    ))}
                    {list.length === 0 && <div className="lane-empty">Nothing here</div>}
                  </div>
                  {group === "done" && more > 0 && (
                    <button className="ghost lane-more" onClick={() => setShowDone(!showDone)}>
                      {showDone ? "Show fewer" : `Show ${more} more`}
                    </button>
                  )}
                </section>
              );
            })}
          </div>
        )}
        {items.length > 0 && side && (
          <p className="lane-keys">
            <span>
              <kbd>j</kbd> <kbd>k</kbd> move in a lane
            </span>
            <span>
              <kbd>h</kbd> <kbd>l</kbd> next lane
            </span>
            <span>
              <kbd>Enter</kbd> open
            </span>
            <span>
              <kbd>Esc</kbd> close
            </span>
            <span className="lane-keys-why">Lanes follow each item's session. Nothing is dragged.</span>
          </p>
        )}
      </main>
    </div>
  );
}

/**
 * What agents filed that nobody has kept or dropped (#252): one tray above the
 * list, every one ticked; Keep or Drop the ticked ones. On a phone it's one line
 * until opened. A proposal's toast opens it.
 */
function FiledTray({ items }: { items: Item[] }) {
  const desktop = useDesktop();
  const focus = useApp((s) => s.focusItem);
  const [open, setOpen] = useState(desktop);
  const [skip, setSkip] = useState<Set<number>>(new Set());
  const ticked = items.filter((i) => !skip.has(i.id));
  const trayRef = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    if (focus === null || !items.some((i) => i.id === focus)) return;
    setOpen(true);
    trayRef.current?.scrollIntoView({ block: "nearest" });
    useApp.setState({ focusItem: null });
  }, [focus, items]);
  const answer = (status: "open" | "dropped") => {
    setStatuses(
      ticked.map((i) => i.id),
      status,
    );
    setSkip(new Set());
  };
  return (
    <section ref={trayRef} className="filed-tray" aria-label="Filed by agents">
      <button className="filed-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="attn-mark attn-filed" aria-hidden />
        <span className="filed-title">{items.length} filed by agents</span>
        <span className="filed-sub">keep what's worth doing, drop the rest</span>
        <Chevron />
      </button>
      {open && (
        <>
          <ul className="filed-list">
            {items.map((i) => (
              <li key={i.id}>
                <label className="filed-row">
                  <input
                    type="checkbox"
                    checked={!skip.has(i.id)}
                    onChange={() =>
                      setSkip((was) => {
                        const next = new Set(was);
                        if (next.has(i.id)) next.delete(i.id);
                        else next.add(i.id);
                        return next;
                      })
                    }
                  />
                  <span className="item-ref">{itemRef(i)}</span>
                  <span className="filed-name">{i.title}</span>
                  <span className="tag">
                    {i.kind} · {i.by}
                  </span>
                </label>
                <button className="ghost filed-edit" aria-label={`Read or edit ${itemRef(i)}`} onClick={() => (location.hash = `#/backlog/${i.id}`)}>
                  Edit
                </button>
              </li>
            ))}
          </ul>
          <div className="order-actions">
            <button className="ghost" disabled={ticked.length === 0} onClick={() => answer("dropped")}>
              Drop {ticked.length}
            </button>
            <button className="ghost" disabled={ticked.length === 0} onClick={() => answer("open")}>
              Keep {ticked.length}
            </button>
          </div>
        </>
      )}
    </section>
  );
}

/**
 * An item in the list (#252): a row with its `api#2`, title, where it stands as a
 * dot and a word, and its tags; a piece sits one step in under its item. Opened,
 * the row's card has the rest and the answers.
 */
function ItemCard({
  item,
  lane,
  depth,
  picked,
  open,
  onToggle,
}: {
  item: Item;
  lane: Lane;
  depth: number;
  picked: boolean;
  open: boolean;
  onToggle: () => void;
}) {
  const now = useApp((s) => s.now);
  const items = useApp((s) => s.items);
  // Its group: the item it's a piece of, or how far its own pieces are.
  const mine = pieces(item, items);
  const [done, of] = progress(item, items);
  const up = item.parent === undefined ? undefined : items.find((i) => i.id === item.parent);
  const group = up ? `piece of #${up.number} ${up.title}` : of > 0 ? `${done}/${of} pieces done` : null;
  const sessions = useApp((s) => s.sessions);
  const boot = useApp((s) => s.boot);
  const expanded = open;
  const setExpanded = (to: boolean) => to !== open && onToggle();
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
  // The supervisor's flag on its session: a red or conflicting PR.
  const flag = useApp((s) => (live ? s.queue.find((q) => q.session === live.id)?.flag?.reason : undefined));
  const where = rowState(item, lane, items, flag);
  // Accept, Open, Launch: Launch beside Open while only takes are live (it hands
  // the item over), Accept beside it for an agent's own filed work.
  const actions = cardActions(item, sessions, boot);
  const pr = prLink(item.pr ?? undefined);

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
  const [draft, setDraft] = useState("");
  const leave = () =>
    act(async () => {
      if (await noteItem(item.id, draft)) setDraft("");
    });
  const who = (by: string) => (by === "human" ? "you" : by);

  return (
    <div className={`item-slot ${picked ? "picked" : ""}`} data-item={item.id} style={depth ? ({ "--depth": depth } as CSSProperties) : undefined}>
      <button className={`row item-row ${picked ? "picked" : ""}`} aria-expanded={expanded} onClick={onToggle}>
        <span className="item-kind" title={item.kind}>
          <KindIcon kind={item.kind} />
        </span>
        <span className="row-main">
          <span className="row-name">{item.title}</span>
          <span className="row-sub">
            <span className={`state-tag state-text-${where.state}`}>{where.word}</span>
            <span className="kind-chip">{item.kind}</span>
            <span className="tag item-meta">
              <span className="item-num">{itemRef(item)}</span>
              {lane !== "done" && (
                <>
                  {" · "}
                  <span className={priority(item) === "p1" ? "item-p1" : undefined}>{priority(item)}</span>
                </>
              )}
              {group && !(depth > 0 && up) && <> · {group}</>}
            </span>
          </span>
        </span>
        <span className="row-age">{age(item.updated * 1000, now)}</span>
      </button>
      {expanded && (
      <article ref={cardRef} className={`card item-card ${EDGE[lane]} ${lane === "done" ? "item-done" : ""}`}>
        {mine.length > 0 && (
          <ul className="item-pieces">
            {mine.map((p) => (
              <li key={p.id} className={p.status === "shipped" || p.status === "dropped" ? "item-piece-done" : undefined}>
                #{p.number} {p.title} <span className="card-age">{p.status}</span>
              </li>
            ))}
          </ul>
        )}
        {agentFiled(item) && <div className="item-live">Filed and taken by {item.by}, not accepted yet</div>}
        {live && (
          <div className="item-live">
            {lane === "needs" && <strong className="item-needs">Needs you</strong>}
            {live.name} · {live.status.state.replace("_", " ")}
          </div>
        )}
        {lane === "review" && pr && (
          <div className="item-live">{item.pr_closed ? "PR closed without merging" : "PR open"}</div>
        )}
        {(
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
          {!live && (
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
      )}
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
        <div className="seg" role="radiogroup" aria-label="Kind">
          {ITEM_KINDS.map((k) => (
            <button
              key={k}
              role="radio"
              aria-checked={kind === k}
              className={`seg-item ${kind === k ? "on" : ""}`}
              onClick={() => setKind(k)}
            >
              {k}
            </button>
          ))}
        </div>

        <div className="seg" role="radiogroup" aria-label="Priority">
          {PRIORITIES.map((p) => (
            <button
              key={p}
              role="radio"
              aria-checked={prio === p}
              className={`seg-item ${prio === p ? "on" : ""}`}
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
