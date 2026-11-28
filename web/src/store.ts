// App state: the connection, the session list and queue as the daemon pushes them,
// the open session's screen, and the prompts waiting in the inbox.

import { create } from "zustand";
import { Conn, type Status } from "./net/conn";
import type { Mismatch } from "./lib/protocol";
import { termColors } from "./lib/theme";
import { glide, moved } from "./lib/glide";
import type {
  Decision,
  ServedTo,
  Note,
  Digest,
  DigestGroup,
  SupervisorEntry,
  WatchedPr,
  PastSession,
  RepoEdit,
  RepoList,
  WikiListing,
  WikiPage,
  SearchResults,
  Doc,
  DeletedDoc,
  DocComment,
  DocVersion,
  Item,
  ItemChange,
  ItemFacts,
  Modes,
  NewItem,
  LaunchOffer,
  Quota,
  LowDisk,
  QueueItem,
  ReviewAction,
  Row,
  ScrollAnchor,
  ServerMsg,
  SessionId,
  SessionInfo,
  Size,
  StepAction,
} from "./proto";
import { applyChat, type Chat } from "./lib/chat";
import { choiceKeys, findChoices, promptContext, type Choice } from "./lib/choices";
import { getJson, upload, type Places } from "./lib/api";
import { itemNo, shippedSummary, wouldShip } from "./lib/backlog";
import { shrinkImage } from "./lib/shrink";
import { newProposals, proposalPopups, proposalToast } from "./lib/proposals";
import { editPatch, landed, overlay, statusDone, type Change, type Lists } from "./lib/optimistic";
import { answeringLabel, batchSummary, choicesKey, type Answering } from "./lib/answering";
import {
  keyBytes,
  messageBytes,
  pathBytes,
  textBytes,
  type Key,
} from "./lib/keys";
import { apply, screenText, type Screen } from "./lib/screen";
import {
  clearNotifications,
  disablePush,
  enablePush,
  pushState,
  testPush,
  type PushState,
} from "./lib/push";

const TOKEN_KEY = "valk.token";
/** The list has no push of its own; the TUI polls it about as often. */
const LIST_EVERY_MS = 2_000;
/** How long a held action's toast offers Undo before the action is sent. */
export const HOLD_MS = 8_000;
/** How long a sent change is drawn over the daemon's lists at most, should its
 * push say something else (another client changed it again). */
const LAND_MS = 5_000;
/** Between keys of one answer, so the program reads them as separate presses. */
const KEY_GAP_MS = 70;
/** How long a confirmed answer keeps the screen's old buttons dim, in case the
 * program never redraws them. */
const ANSWERED_DIM_MS = 4000;
/** Between a message and its Enter: long enough that the two can't arrive as one read. */
const ENTER_GAP_MS = 150;

export interface Prompt {
  seq: number;
  choices: Choice[];
  /** What it asks about, read off the screen (the command, the file). */
  context: string | null;
}

interface State {
  token: string | null;
  status: Status;
  /** Set while `outdated`: the daemon's protocol and this app's. */
  mismatch: Mismatch | null;
  sessions: SessionInfo[];
  queue: QueueItem[];
  /** Decisions agents proposed, waiting on the human (ADR-0007). */
  proposals: Decision[];
  /** Counts the daemon's proposal pushes: one lands after an accept takes effect. */
  proposalPushes: number;
  /** Docs an agent wrote since the Docs list was last opened (ADR-0021 §5): a count, never cards. */
  unreadDocs: number;
  /** The backlog (DESIGN §8.12), as the daemon pushes it. */
  items: Item[];
  /** Ready items the supervisor offers to start (ADR-0019 §2), pushed with the items. */
  launches: LaunchOffer[];
  /** The plan's quota (protocol 30), pushed with the queue. */
  quota: Quota | null;
  /** The disks running low on space (#300), pushed with the queue, and when this
   * app first saw one low (ms). */
  disk: { low: LowDisk[]; since: number };
  /** The daemon's `boot`: an item's session ids only mean something on the same one. */
  boot: number;
  /** The session on screen, its screen, and its exit once it has one. */
  openId: SessionId | null;
  screen: Screen | null;
  exited: { code: number | null } | null;
  /** The open session's agent conversation, from its transcript; `null` until it loads. */
  chat: Chat | null;
  /** Choices on the screens of queued sessions, by session, for the inbox. */
  prompts: Record<SessionId, Prompt>;
  /** Answers sent and not yet confirmed, or confirmed while the screen catches up. */
  answering: Record<SessionId, Answering>;
  /** `action`: a button on it, such as Undo; the toast stays longer. A backlog
   * proposal's (#194) has `actions` (Accept, Decline), `open` for a tap on its
   * text, and its own `ms`. */
  toast: {
    text: string;
    at: number;
    action?: { label: string; run: () => void };
    actions?: { label: string; run: () => void }[];
    open?: () => void;
    ms?: number;
  } | null;
  /** The backlog card to open and scroll to, once, when the backlog shows. */
  focusItem: number | null;
  now: number;
  push: PushState;
  /** Turning notifications on: the browser's push service can take a while. */
  pushBusy: boolean;
  /** Android's install prompt, kept for an Install button. */
  canInstall: boolean;
}

export const useApp = create<State>(() => ({
  token: localStorage.getItem(TOKEN_KEY),
  status: "connecting",
  mismatch: null,
  sessions: [],
  queue: [],
  proposals: [],
  proposalPushes: 0,
  unreadDocs: 0,
  items: [],
  launches: [],
  quota: null,
  disk: { low: [], since: 0 },
  boot: 0,
  openId: null,
  screen: null,
  exited: null,
  chat: null,
  prompts: {},
  answering: {},
  toast: null,
  focusItem: null,
  now: Date.now(),
  push: "unknown",
  pushBusy: false,
  canInstall: false,
}));

const set = useApp.setState;
const get = useApp.getState;
type Action = { label: string; run: () => void };
let conn: Conn | null = null;
/** The page-leaving listeners are added, once. */
let leaving = false;
let listTimer: number | null = null;

type Raised = Omit<NonNullable<State["toast"]>, "at">;

export function toast(text: string, action?: Action): void {
  raise({ text, action });
}

function raise(t: Raised): void {
  // Any toast waits while an Undo is offered: a reply to an earlier action
  // mustn't take the Undo off this one and send it. Only another hold does.
  if (held) {
    deferred = t;
    return;
  }
  set({ toast: { ...t, at: Date.now() } });
}

/** The last toast raised while an Undo was offered, shown once it's gone. */
let deferred: Raised | null = null;

function showDeferred(): void {
  const d = deferred;
  deferred = null;
  if (d) raise(d);
}

/** How long a proposal's toast stays: long enough to read it and answer. */
const PROPOSAL_MS = 12_000;
/** The backlog's ids at the last items push; `null` until the first, which primes. */
let knownItems: Set<number> | null = null;

/** Agents' new backlog proposals, as a toast to accept or decline (#194). */
function popProposals(items: Item[]): void {
  const { known, fresh } = newProposals(knownItems, items);
  knownItems = known;
  const t = proposalPopups() ? proposalToast(fresh) : null;
  if (!t) return;
  raise({
    text: t.text,
    ms: PROPOSAL_MS,
    open: () => openBacklogAt(t.id),
    actions: [
      { label: "Decline", run: () => answerProposal(t.id, "dropped") },
      { label: "Accept", run: () => answerProposal(t.id, "open") },
    ],
  });
}

/** Answers a proposal's toast, unless it was answered elsewhere meanwhile. */
function answerProposal(id: number, status: Item["status"]): void {
  if (get().items.find((i) => i.id === id)?.status !== "proposed") return toast(`#${itemNo(get().items, id)} was already answered`);
  setStatus(id, status);
}

/** The backlog, its card `id` open and in view. */
export function openBacklogAt(id: number): void {
  // The toast goes too: the page change remounts it, which would show it again.
  set({ focusItem: id, toast: null });
  location.hash = "#/backlog";
}

// Optimistic actions (backlog #185). The daemon's lists are kept as it pushed
// them; what the store shows is them with the user's pending changes on top.
// The store's own empty lists, so the first push compares against what's drawn.
let raw: Lists = { proposals: get().proposals, items: get().items, queue: get().queue };
/** Changes by key; `sent` once the daemon took it, null while held or in flight. */
const shownOver = new Map<number, { change: Change; sent: number | null }>();
let pendingKey = 0;

/** Redraws the lists, first forgetting sent changes the daemon shows (or that
 * have had long enough to show). */
function show(): void {
  const now = Date.now();
  for (const [key, p] of shownOver) {
    if (p.sent !== null && (landed(raw, p.change) || now - p.sent >= LAND_MS)) shownOver.delete(key);
  }
  const shown = overlay(raw, [...shownOver.values()].map((p) => p.change));
  set({ proposals: shown.proposals, items: shown.items, queue: shown.queue });
}

type Send = () => Promise<string | null | void>;

/** Sends a change shown already; taken back with a toast if the daemon refuses
 * it, carrying `retry` when there's a way to try again. */
async function commit(keys: number[], send: Send, retry?: Action): Promise<void> {
  try {
    if (!conn) throw new Error("not connected");
    const said = await send();
    for (const key of keys) {
      const p = shownOver.get(key);
      if (p) p.sent = Date.now();
    }
    window.setTimeout(show, LAND_MS);
    show();
    if (said) toast(said);
  } catch (e) {
    for (const key of keys) shownOver.delete(key);
    show();
    toast(`Couldn't: ${(e as Error).message}`, retry);
  }
}

/** The action waiting out its Undo; one at most, as there's one toast. */
let held: { keys: number[]; send: Send; retry?: Action; timer: number } | null = null;

/** Sends the held action now: its Undo is gone, or the page is going away.
 * What waited for it is shown, unless another hold takes the toast (`next`). */
function flushHeld(next = false): void {
  const h = held;
  if (!h) return;
  held = null;
  window.clearTimeout(h.timer);
  void commit(h.keys, h.send, h.retry);
  if (!next) showDeferred();
}

/**
 * Shows `change` at once and offers Undo in the toast; `send` goes to the
 * daemon only when the Undo does (the toast times out or another replaces it,
 * the tab is hidden). Undo before then and the daemon never hears of it, which
 * is what lets this cover actions with no inverse: an accept, a delete. Several
 * changes share the one toast and Undo: a batch is answered as one.
 */
function hold(text: string, change: Change | Change[], send: Send, retry?: Action): void {
  // Another Undo takes this one's place, so what it would have undone goes now.
  flushHeld(true);
  const keys = [change].flat().map((c) => {
    const key = ++pendingKey;
    shownOver.set(key, { change: c, sent: null });
    return key;
  });
  show();
  toast(text, {
    label: "Undo",
    run: () => {
      if (held?.keys !== keys) return toast("Too late: it was sent");
      window.clearTimeout(held.timer);
      held = null;
      for (const key of keys) shownOver.delete(key);
      show();
      showDeferred();
    },
  });
  navigator.vibrate?.(10);
  held = { keys, send, retry, timer: window.setTimeout(flushHeld, HOLD_MS) };
}

/** Tries the daemon again now, rather than after the backoff. */
export function reconnect(): void {
  conn?.retryNow();
}

export function saveToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
  set({ token, status: "connecting" });
}

export function forgetToken(): void {
  localStorage.removeItem(TOKEN_KEY);
  conn?.stop();
  conn = null;
  set({ token: null, status: "unpaired" });
}

/**
 * Loads the server's newer app: now, unless you're typing into a field (a rename,
 * a decision being edited), else as soon as you leave it or the app. The pill
 * offers a reload meanwhile.
 */
function reloadWhenSafe(): void {
  const tryNow = () => {
    const el = document.activeElement;
    const editing =
      el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
    if (!editing || document.visibilityState === "hidden") location.reload();
  };
  tryNow();
  // After focus has moved on, so activeElement is the new one.
  document.addEventListener("focusout", () => window.setTimeout(tryNow, 0));
  document.addEventListener("visibilitychange", tryNow);
}

export function connect(): void {
  const token = get().token;
  if (!token || conn) return;
  conn = new Conn({
    token,
    onStatus: (status, mismatch) => {
      set({ status, mismatch: mismatch ?? null });
      if (status === "updated") reloadWhenSafe();
    },
    onPush,
    onReady: (boot) => {
      set({ boot });
      reportVisible();
      void conn?.request({ t: "watch_queue" }).catch(() => {});
      void refresh();
      const open = get().openId;
      if (open !== null) void attach(open);
      // The server forgets what a dropped connection followed.
      if (open !== null) conn?.send({ t: "chat", session: open });
    },
  });
  conn.start();
  document.addEventListener("visibilitychange", reportVisible);
  if (!leaving) {
    leaving = true;
    // Leaving the app is leaving the Undo: what's held goes before the page can,
    // and the toast stops offering it.
    const leave = () => {
      if (!held) return;
      flushHeld();
      if (get().toast?.action?.label === "Undo") set({ toast: null });
    };
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") leave();
    });
    window.addEventListener("pagehide", leave);
  }
  void checkPush();
  if (listTimer === null) {
    listTimer = window.setInterval(() => {
      set({ now: Date.now() });
      if (document.visibilityState === "visible") void refresh();
    }, LIST_EVERY_MS);
  }
}

function onPush(msg: ServerMsg): void {
  switch (msg.t) {
    case "queue":
      raw = { ...raw, queue: msg.items };
      glide(show, moved(get().queue.map((q) => q.session), msg.items.map((q) => q.session)));
      void refreshPrompts(msg.items);
      void refresh();
      break;
    case "proposals":
      raw = { ...raw, proposals: msg.items };
      set({ proposalPushes: get().proposalPushes + 1 });
      show();
      break;
    case "items":
      popProposals(msg.items);
      raw = { ...raw, items: msg.items };
      set({ launches: msg.launches ?? [] });
      show();
      break;
    case "quota":
      set({ quota: msg.quota });
      break;
    case "disk": {
      const was = get().disk;
      const since = msg.low.length === 0 ? 0 : was.low.length > 0 ? was.since : Date.now();
      set({ disk: { low: msg.low, since } });
      break;
    }
    case "screen":
      if (msg.session === get().openId) showScreen(apply(latest(), msg.update));
      break;
    case "chat":
      if (msg.session === get().openId)
        set({ chat: applyChat(get().chat, msg) });
      break;
    case "exited":
      if (msg.session === get().openId) set({ exited: { code: msg.code } });
      break;
    case "clipboard":
      void navigator.clipboard?.writeText(msg.text).then(
        () => toast(`copied ${msg.text.length} chars`),
        () => {},
      );
      break;
  }
}

// The open session's screen is shown once a frame: a burst of output (a build log,
// `cat` of a big file) arrives as many updates between two paints, and rendering
// each would only fall behind. Every update is still applied, in order.
let pending: Screen | null = null;
let frame: { raf: number; timer: number } | null = null;
/** A window that's covered gets a frame a second or none; the screen still moves on. */
const FRAME_LATEST_MS = 100;

function latest(): Screen | null {
  return frame === null ? get().screen : pending;
}

function showScreen(screen: Screen): void {
  pending = screen;
  if (frame !== null) return;
  const paint = () => {
    const shown = pending;
    clearScreen();
    set({ screen: shown });
  };
  frame = { raf: requestAnimationFrame(paint), timer: window.setTimeout(paint, FRAME_LATEST_MS) };
}

/** Forgets the screen on show, and any update still waiting for its frame. */
function clearScreen(): void {
  if (frame !== null) {
    cancelAnimationFrame(frame.raf);
    clearTimeout(frame.timer);
  }
  frame = null;
  pending = null;
}

/** Tells the server whether this app is on screen: notifications wait while it is. */
function reportVisible(): void {
  const visible = document.visibilityState === "visible";
  conn?.send({ t: "visible", visible });
  if (visible) void clearNotifications();
}

export async function checkPush(): Promise<void> {
  const token = get().token;
  if (token)
    set({ push: await pushState(token).catch(() => "unsupported" as const) });
}

export async function turnOnPush(): Promise<void> {
  const token = get().token;
  if (!token || get().pushBusy) return;
  set({ pushBusy: true });
  try {
    const on = await enablePush(token);
    set({
      push: on ? "on" : Notification.permission === "denied" ? "denied" : "off",
    });
    if (on) toast("Notifications on");
  } catch (e) {
    toast(`Couldn't turn on notifications: ${(e as Error).message}`);
  } finally {
    set({ pushBusy: false });
  }
}

export async function turnOffPush(): Promise<void> {
  const token = get().token;
  if (!token) return;
  await disablePush(token).catch(() => {});
  set({ push: "off" });
  toast("Notifications off");
}

export async function sendTestPush(): Promise<void> {
  const token = get().token;
  if (!token) return;
  try {
    const d = await testPush(token);
    toast(
      d.sent
        ? "Sent. It should arrive in a moment"
        : d.errors.length
          ? `Not sent: ${d.errors[0]}`
          : "Not sent: this device isn't subscribed",
    );
  } catch (e) {
    toast(`Test failed: ${(e as Error).message}`);
  }
}

let installEvent: (Event & { prompt: () => Promise<void> }) | null = null;

window.addEventListener("beforeinstallprompt", (e) => {
  e.preventDefault();
  installEvent = e as typeof installEvent;
  set({ canInstall: true });
});

export async function install(): Promise<void> {
  await installEvent?.prompt();
  installEvent = null;
  set({ canInstall: false });
}

export async function refresh(): Promise<void> {
  try {
    const reply = await conn?.request({ t: "list" });
    if (reply?.t === "sessions") {
      const next = reply.sessions;
      glide(
        () => set({ sessions: next }),
        moved(get().sessions.map((x) => x.id), next.map((x) => x.id)),
      );
    }
  } catch {
    /* the connection reports itself */
  }
}

/** Reads the choices off each waiting session's screen, when its turn changed. */
async function refreshPrompts(items: QueueItem[]): Promise<void> {
  const prompts: Record<SessionId, Prompt> = {};
  for (const item of items) {
    if (item.status.state !== "needs_input") continue;
    const known = get().prompts[item.session];
    if (known && known.seq === item.status.seq && known.choices.length) {
      prompts[item.session] = known;
      continue;
    }
    try {
      const reply = await conn?.request({ t: "dump", session: item.session });
      if (reply?.t !== "text") continue;
      const lines = reply.text.split("\n");
      prompts[item.session] = {
        seq: item.status.seq,
        choices: findChoices(lines),
        context: promptContext(lines),
      };
    } catch {
      /* shown without buttons */
    }
  }
  set({ prompts });
}

export async function open(id: SessionId): Promise<void> {
  if (get().openId === id) return;
  clearScreen();
  set({ openId: id, screen: null, exited: null, chat: null });
  // Followed whichever view shows, so switching to Chat is instant.
  conn?.send({ t: "chat", session: id });
  await attach(id);
}

async function attach(id: SessionId): Promise<void> {
  // Not connected yet: the handshake attaches the open session when it's done.
  if (conn?.status !== "open") return;
  try {
    // No size: watching from a phone must not resize the desktop's session.
    await conn?.request({ t: "attach", session: id, size: null });
  } catch (e) {
    toast(`can't open: ${(e as Error).message}`);
  }
}

export async function close(): Promise<void> {
  if (get().openId === null) return;
  clearScreen();
  set({ openId: null, screen: null, exited: null, chat: null });
  conn?.send({ t: "chat", session: null });
  await conn?.request({ t: "detach" }).catch(() => {});
}

function input(session: SessionId, data: number[]): boolean {
  const ok = conn?.send({ t: "input", session, data }) ?? false;
  if (!ok) toast("not connected; nothing was sent");
  return ok;
}

/** Keys typed into the open session's terminal, already as the bytes to send. */
export function sendBytes(data: number[]): boolean {
  const id = get().openId;
  return id !== null && data.length > 0 && input(id, data);
}

/** Bytes for one session, whichever is open: what's owed to the one you just left. */
export function sendBytesTo(session: SessionId, data: number[]): void {
  conn?.send({ t: "input", session, data });
}

/** The modes of the open session's program as last heard, ahead of the frame that shows them. */
export function openModes(): Modes | undefined {
  return latest()?.modes;
}

/**
 * Sizes the open session to the terminal you're typing in. Only the desktop
 * terminal does, once it has the keyboard: watching never resizes a session.
 */
export function resizeOpen(size: Size): void {
  const id = get().openId;
  if (id !== null) conn?.send({ t: "resize", session: id, size });
}

/** A screenful of the open session's history, or `null` when it can't be had. */
export async function scrollback(
  anchor: ScrollAnchor,
): Promise<{ from_top: number; history: number; rows: Row[] } | null> {
  const id = get().openId;
  if (id === null || !conn) return null;
  try {
    const reply = await conn.request({ t: "scrollback", session: id, anchor });
    return reply.t === "scrollback" && get().openId === id ? reply : null;
  } catch {
    return null;
  }
}

export function sendKey(key: Key): void {
  const id = get().openId;
  if (id !== null) input(id, keyBytes(key, get().screen?.modes));
}

/**
 * Types a message, then presses Enter once the program has read it as typing. Files
 * go first, each path on its own, which Claude Code and Codex attach as an image
 * when it is one; other files they read by path.
 */
function message(
  session: SessionId,
  text: string,
  modes?: Modes,
  paths: string[] = [],
): boolean {
  const parts = paths.flatMap((p) => [pathBytes(p, modes), textBytes(" ")]);
  if (text.trim()) parts.push(messageBytes(text, modes));
  parts.push(keyBytes("enter", modes));
  const [first, ...rest] = parts;
  if (!input(session, first)) return false;
  rest.forEach((part, i) =>
    setTimeout(() => input(session, part), ENTER_GAP_MS * (i + 1)),
  );
  return true;
}

export function sendMessage(text: string, paths: string[] = []): boolean {
  const id = get().openId;
  if (id === null || (!text.trim() && paths.length === 0)) return false;
  return message(id, text, get().screen?.modes, paths);
}

/**
 * Sends a file to the session's machine, for a message to hand to the agent; a
 * photo first made small, and a JPEG if the agents can't attach its format.
 */
export async function sendFile(file: File): Promise<string> {
  const token = get().token;
  if (!token) throw new Error("pair this device first");
  return upload(token, await shrinkImage(file));
}

/** Marks an answer under way, its buttons dim at once; false if this prompt
 * has one already. A new prompt on the screen takes a tap straight away. */
function startAnswer(session: SessionId, choices: Choice[], choice: Choice): boolean {
  const seq = statusSeq(session);
  if (answeringLabel(get().answering[session], choices, seq) !== null) return false;
  const a: Answering = { of: choicesKey(choices), label: choice.label, done: false, seq };
  set({ answering: { ...get().answering, [session]: a } });
  return true;
}

/** The session's status `seq`, which moves on with each prompt. */
export function statusSeq(session: SessionId): number | null {
  const s = get();
  return (
    s.sessions.find((x) => x.id === session)?.status.seq ??
    s.queue.find((q) => q.session === session)?.status.seq ??
    null
  );
}

/** Ends `a` if it's still the session's answer: a later one keeps its own. */
function endAnswer(session: SessionId, a: Answering | undefined): void {
  if (!a || get().answering[session] !== a) return;
  const answering = { ...get().answering };
  delete answering[session];
  set({ answering });
}

/** Picks a choice: moves the program's cursor to it, then Enter. The buttons
 * show it sent at once and stay that way unless it doesn't go through. */
export async function answer(
  session: SessionId,
  choices: Choice[],
  choice: Choice,
): Promise<void> {
  if (!startAnswer(session, choices, choice)) return;
  await sendAnswer(session, choices, choice);
}

/** Sends an answer `startAnswer` marked, then confirms or takes it back. */
async function sendAnswer(
  session: SessionId,
  choices: Choice[],
  choice: Choice,
  quiet = false,
): Promise<boolean> {
  const mine = get().answering[session];
  const failed = () => {
    endAnswer(session, mine);
    if (!quiet) toast("Didn't go through; try again");
    return false;
  };
  const keys = choiceKeys(choices, choice);
  const modes = get().openId === session ? get().screen?.modes : undefined;
  navigator.vibrate?.(10);
  if (keys === null) {
    // No cursor to move: the number picks it.
    const n = choice.n ?? choices.indexOf(choice) + 1;
    if (!input(session, [...new TextEncoder().encode(String(n))])) return failed();
  } else {
    for (const [i, key] of keys.entries()) {
      if (i) await new Promise((r) => setTimeout(r, KEY_GAP_MS));
      if (!input(session, keyBytes(key, modes))) return failed();
    }
  }
  // Keystrokes are fire-and-forget, and a phone just back from sleep can hold a
  // socket that says it's open but is dead. The daemon handles a connection's
  // messages in order, so a reply to this means the keys reached the program.
  try {
    if (!conn) throw new Error("not connected");
    await conn.request({ t: "hello" });
  } catch {
    return failed();
  }
  // Answered: drop the inbox's buttons until the screen says otherwise. The
  // open session's dock reads the live screen, so its old buttons stay dim
  // until the program redraws them, or a few seconds pass.
  const prompts = { ...get().prompts };
  delete prompts[session];
  if (mine && get().answering[session] === mine) {
    const done = { ...mine, done: true };
    set({ prompts, answering: { ...get().answering, [session]: done } });
    window.setTimeout(() => endAnswer(session, done), ANSWERED_DIM_MS);
  } else {
    set({ prompts });
  }
  if (!quiet) toast(`${choice.label.slice(0, 40)} ✓`);
  return true;
}

/** Takes a finished session off the queue at once; the daemon hears right away. */
export async function markSeen(item: QueueItem): Promise<void> {
  const key = ++pendingKey;
  shownOver.set(key, { change: { t: "seen", session: item.session, seq: item.status.seq }, sent: null });
  show();
  try {
    if (!conn) throw new Error("not connected");
    await conn.request({ t: "mark_seen", session: item.session, seq: item.status.seq });
    const p = shownOver.get(key);
    if (p) p.sent = Date.now();
    window.setTimeout(show, LAND_MS);
  } catch {
    // Not seen after all: it stays in the queue, as it would have without this.
    shownOver.delete(key);
  }
  show();
}

export async function rename(
  id: SessionId,
  name: string | null,
): Promise<void> {
  await conn
    ?.request({ t: "rename", session: id, name })
    .catch((e) => toast(String(e.message)));
  void refresh();
}

export async function kill(id: SessionId): Promise<void> {
  await conn
    ?.request({ t: "kill", session: id })
    .catch((e) => toast(String(e.message)));
  void refresh();
}

const REVIEWED: Record<ReviewAction["a"], string> = {
  accept: "Accepted",
  reject: "Rejected",
  retire: "Retired",
  restore: "Restored",
  reopen: "Reopened",
  confirm: "Confirmed",
  pin: "Pinned",
  unpin: "Unpinned",
  edit: "Saved",
  revise: "Accepted",
  scope: "Scoped",
  propose_scope: "Scope proposed for",
  decline_scope: "Kept everywhere:",
};

async function sendReview(d: Decision, action: ReviewAction): Promise<void> {
  if (!conn) throw new Error("not connected");
  await conn.request({
    t: "review",
    project: d.project,
    id: d.id,
    action,
    seen: d.updated,
  });
}

/** Accepts or rejects a proposal at once, with Undo; the daemon hears when that's
 * gone, then `sent` runs. */
export function decide(d: Decision, a: "accept" | "reject", sent?: () => void): void {
  hold(`${REVIEWED[a]} #${d.id}`, { t: "decided", project: d.project, id: d.id }, async () => {
    await sendReview(d, { a });
    sent?.();
  });
}

/** Rewords, retires or otherwise reviews a decision; the daemon pushes the new list. */
export async function review(
  d: Decision,
  action: ReviewAction,
): Promise<boolean> {
  try {
    await sendReview(d, action);
    navigator.vibrate?.(10);
    toast(`${REVIEWED[action.a]} #${d.id} ✓`);
    return true;
  } catch (e) {
    toast(`Couldn't: ${(e as Error).message}`);
    return false;
  }
}

/** Every project's decisions and your habits, as the daemon has them now; null
 * when it can't be asked. Not pushed: a page asks again after it changes one. */
/** The sessions decision `d` reached lately, the latest first (backlog #256).
 * Throws with the daemon's reason. */
export async function servedTo(d: Decision): Promise<ServedTo[]> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "served_to", project: d.project, id: d.id });
  if (reply.t !== "served_to") throw new Error("unexpected reply");
  return reply.sessions;
}

export async function allDecisions(): Promise<Decision[] | null> {
  try {
    if (!conn) throw new Error("not connected");
    const reply = await conn.request({ t: "decisions", cwd: null });
    return reply.t === "decisions" ? reply.decisions : null;
  } catch {
    return null;
  }
}

/** Every project's agent notes, every status; null when they can't be asked. */
export async function allNotes(): Promise<Note[] | null> {
  try {
    if (!conn) throw new Error("not connected");
    const reply = await conn.request({ t: "notes", cwd: null });
    return reply.t === "notes" ? reply.listing.notes : null;
  } catch {
    return null;
  }
}

/** Corrects note `n` with `text`: the new note replaces it in one write. */
export async function correctNote(n: Note, text: string): Promise<boolean> {
  try {
    if (!conn) throw new Error("not connected");
    const reply = await conn.request({ t: "note", note: { cwd: n.project, text, replaces: n.id } });
    toast(`Replaced note #${n.id}${reply.t === "note" ? ` with #${reply.note.id}` : ""} ✓`);
    return true;
  } catch (e) {
    toast(`Couldn't: ${(e as Error).message}`);
    return false;
  }
}

/** Deletes note `n`; its tombstone keeps it from being written again. */
export async function deleteNote(n: Note): Promise<boolean> {
  try {
    if (!conn) throw new Error("not connected");
    await conn.request({ t: "note_rm", cwd: n.project, id: n.id });
    toast(`Deleted note #${n.id} ✓`);
    return true;
  } catch (e) {
    toast(`Couldn't: ${(e as Error).message}`);
    return false;
  }
}

/** Every doc, newest first, and how many an agent wrote since the list was last
 * opened (ADR-0021 §5); null when it can't be asked. */
export async function allDocs(): Promise<{ docs: Doc[]; unread: number; at: number } | null> {
  try {
    if (!conn) throw new Error("not connected");
    const reply = await conn.request({ t: "docs" });
    return reply.t === "docs" ? { docs: reply.docs, unread: reply.unread ?? 0, at: reply.at ?? 0 } : null;
  } catch {
    return null;
  }
}

/** The docs session `id` wrote, newest first (backlog #311). Throws with the daemon's reason. */
export async function sessionDocs(id: SessionId): Promise<Doc[]> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "docs", session: id });
  if (reply.t !== "docs") throw new Error("unexpected reply");
  return reply.docs;
}

/** What every agent did since `sinceMs`, or since you were last gone (backlog
 * #105). Throws with the daemon's reason. */
export async function awayDigest(sinceMs: number | null, by: DigestGroup): Promise<Digest> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "digest", since_ms: sinceMs, by });
  if (reply.t !== "digest") throw new Error("unexpected reply");
  return reply.digest;
}

/** What the supervisor did, newest first, and whether it's on (backlog #180).
 * Throws with the daemon's reason. */
export async function supervisorLog(): Promise<{ entries: SupervisorEntry[]; on: boolean }> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "supervisor_log" });
  if (reply.t !== "supervisor_log") throw new Error("unexpected reply");
  return { entries: reply.entries, on: reply.on };
}

/** Whether the daemon keeps the web app running on localhost (backlog #316);
 * `on` sets it first. Throws with the daemon's reason. */
export async function webServer(on: boolean | null = null): Promise<boolean> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "web_server", on });
  if (reply.t !== "text") throw new Error("unexpected reply");
  return reply.text === "on";
}

/** The pull requests the supervisor watches, as its poll last read them, and
 * when it reads them next (backlog #181). Throws with the daemon's reason. */
export async function supervisorWatch(): Promise<{ prs: WatchedPr[]; polled_at?: number; next_at?: number; on: boolean }> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "supervisor_watch" });
  if (reply.t !== "supervisor_watch") throw new Error("unexpected reply");
  return { prs: reply.prs, polled_at: reply.polled_at ?? undefined, next_at: reply.next_at ?? undefined, on: reply.on };
}

/** Opens the supervisor session, starting it if none runs (ADR-0019 §10), in
 * the daemon's default directory. Throws with the daemon's reason. */
export async function supervisorSession(): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "supervisor_session", cwd: "", size: SPAWN_SIZE, env: [] });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

/** Agent sessions that have ended, newest first (backlog #150). Throws with the
 * daemon's reason. */
export async function pastSessions(): Promise<PastSession[]> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "history" });
  if (reply.t !== "history") throw new Error("unexpected reply");
  return reply.sessions;
}

/** Starts the past session `key` again and returns the session it runs in.
 * Throws with the daemon's reason. */
export async function resume(key: string): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "resume", key, size: SPAWN_SIZE, env: [] });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

/** The repositories every picker offers (backlog #240). */
export async function repoList(): Promise<RepoList> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "repos" });
  if (reply.t !== "repos") throw new Error("unexpected reply");
  return reply.list;
}

/** Changes which repositories pickers offer; the list as it is after. Throws
 * with the daemon's reason (not a git repository, no such folder). */
export async function repoEdit(edit: RepoEdit): Promise<RepoList> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "repo_edit", edit });
  if (reply.t !== "repos") throw new Error("unexpected reply");
  return reply.list;
}

/** The wiki of `cwd`'s repository (backlog #210). Throws with the daemon's reason. */
export async function wikiListing(cwd: string): Promise<WikiListing> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "wiki", cwd });
  if (reply.t !== "wiki") throw new Error("unexpected reply");
  return reply.listing;
}

/** One wiki page in full. Throws with the daemon's reason. */
export async function wikiPage(cwd: string, page: string): Promise<WikiPage> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "wiki_page", cwd, page });
  if (reply.t !== "wiki_page") throw new Error("unexpected reply");
  return reply.page;
}

/** Starts the agent that maps `cwd`'s repository into its wiki (with `stale`,
 * rewrites only the stale pages) and returns its session: the running one, if
 * one is already at it. Throws with the daemon's reason. */
export async function wikiExplore(cwd: string, stale: boolean): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "wiki_explore", cwd, stale, agent: "claude", size: SPAWN_SIZE, env: [] });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

/** Starts the tutor, an agent that teaches Valkyrie one lesson at a time, and
 * returns its session: the running one, if one is already teaching. Throws with
 * the daemon's reason. */
export async function startTutor(): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "tutor", agent: "claude", size: SPAWN_SIZE, env: [] });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

/** The sessions whose history since `sinceMs` holds every word of `query`,
 * newest first (backlog #106). Throws with the daemon's reason. */
export async function searchHistory(query: string, sinceMs: number | null, limit = 30): Promise<SearchResults> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "search", query, since_ms: sinceMs, limit });
  if (reply.t !== "search") throw new Error("unexpected reply");
  return reply.results;
}

/** One doc's text, by the key its list entry carries. Throws with the daemon's reason. */
export async function openDoc(key: string): Promise<Doc> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "open_doc", key });
  if (reply.t !== "doc") throw new Error("unexpected reply");
  return reply.doc;
}

/** A doc's comments (ADR-0021 §6); none on one without a valk-id. Throws with the daemon's reason. */
export async function docComments(key: string): Promise<DocComment[]> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "doc_comments", key });
  if (reply.t !== "doc_comments") throw new Error("unexpected reply");
  return reply.comments;
}

/** Comments on a doc, or on the block `quote` reads. Returns them all, and the
 * running session that hears it at its next prompt, if any. */
export async function commentDoc(
  key: string,
  quote: string | null,
  text: string,
): Promise<{ comments: DocComment[]; to?: string }> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "comment_doc", key, text, ...(quote ? { quote } : {}) });
  if (reply.t !== "doc_comments") throw new Error("unexpected reply");
  return { comments: reply.comments, to: reply.to ?? undefined };
}

/** Closes a comment; returns the doc's comments. */
export async function resolveComment(key: string, comment: number): Promise<DocComment[]> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "resolve_comment", key, comment });
  if (reply.t !== "doc_comments") throw new Error("unexpected reply");
  return reply.comments;
}

/** A save or restore: the doc as written, or with `conflict`, as it is on disk now. */
export type Saved = { doc: Doc; conflict: boolean };

/** The user's save from the editor (ADR-0021 §7), over the version whose hash
 * `base` is. Throws with the daemon's reason. */
export async function saveDoc(key: string, base: string, body: string): Promise<Saved> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "save_doc", key, base, body });
  if (reply.t !== "doc_saved") throw new Error("unexpected reply");
  return { doc: reply.doc, conflict: !!reply.conflict };
}

/** A doc's past versions, newest first, and one's whole text when asked. */
export async function docHistory(key: string, version?: string): Promise<{ versions: DocVersion[]; text?: string }> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "doc_history", key, ...(version ? { version } : {}) });
  if (reply.t !== "doc_history") throw new Error("unexpected reply");
  return { versions: reply.versions, text: reply.text ?? undefined };
}

/** Puts a past version back over the version whose hash `base` is. */
export async function restoreDoc(key: string, base: string, version: string): Promise<Saved> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "restore_doc", key, base, version });
  if (reply.t !== "doc_saved") throw new Error("unexpected reply");
  return { doc: reply.doc, conflict: !!reply.conflict };
}

/** Deletes a doc if `base` is still on disk: the token that puts it back, or
 * the doc as it is now if someone saved since. */
export async function deleteDoc(key: string, base: string): Promise<{ token: string } | { conflict: Doc }> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "delete_doc", key, base });
  if (reply.t === "doc_deleted") return { token: reply.token };
  if (reply.t === "doc_saved") return { conflict: reply.doc };
  throw new Error("unexpected reply");
}

/** Deleted docs that can still be put back, newest first; none when it can't be asked. */
export async function deletedDocs(): Promise<DeletedDoc[]> {
  try {
    if (!conn) return [];
    const reply = await conn.request({ t: "deleted_docs" });
    return reply.t === "deleted_docs" ? reply.docs : [];
  } catch {
    return [];
  }
}

/** Puts a deleted doc back where it was. */
export async function undeleteDoc(token: string): Promise<Doc> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "undelete_doc", token });
  if (reply.t !== "doc") throw new Error("unexpected reply");
  return reply.doc;
}

/** The Docs list listed `at` is open: clear the Inbox's unread count. What came
 * in after `at` stays unread, since the list didn't show it. */
export function docsSeen(at: number): void {
  set({ unreadDocs: 0 });
  void conn?.request({ t: "docs_seen", at }).catch(() => undefined);
}

/** Asks how many docs are unread, for the Inbox's count: the count alone, not the list. */
export async function refreshUnreadDocs(): Promise<void> {
  try {
    if (!conn) return;
    const reply = await conn.request({ t: "docs", count_only: true });
    if (reply.t === "docs") set({ unreadDocs: reply.unread ?? 0 });
  } catch {
    // Not connected, or the daemon can't be asked: keep the last count.
  }
}

/** The choices on the open session's screen right now. */
export function openChoices(screen: Screen | null): Choice[] {
  return screen ? findChoices(screenText(screen)) : [];
}

/** Tells a session we started the colors it is shown in, so its programs can tell a light look from a dark one. */
function reportColors(session: SessionId): void {
  const colors = termColors();
  if (colors) conn?.send({ t: "term_colors", session, colors });
}

/** The size a new session starts at; a TUI that attaches later resizes it. */
const SPAWN_SIZE = { cols: 120, rows: 40 };

/** Starts a session (DESIGN §8.8) and returns its id. */
export async function spawn(
  command: string[],
  cwd: string,
  name: string | null,
  worktree: string | null = null,
): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const spec = { command, cwd, name, size: SPAWN_SIZE, env: [], worktree };
  const reply = await conn.request({ t: "spawn", spec });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

let home: Promise<string> | null = null;

/** This machine's home folder, where a session with no repository starts. */
function homeDir(): Promise<string> {
  const token = useApp.getState().token;
  if (!token) return Promise.reject(new Error("not signed in"));
  home ??= getJson<Places>(token, "/api/places").then(
    (p) => p.home,
    (e: Error) => {
      home = null;
      throw e;
    },
  );
  return home;
}

/**
 * Starts a typed task: in a worktree of its own of `repo`, or in the home
 * folder when it names none (backlog #292). Returns the session's id.
 */
export async function startTask(command: string[], repo: string | null, name: string): Promise<SessionId> {
  return repo ? spawn(command, repo, name, "") : spawn(command, await homeDir(), name, null);
}

/** Writes down a backlog item; its id. */
export async function addItem(item: NewItem): Promise<number> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "add_item", item });
  if (reply.t !== "item") throw new Error("unexpected reply");
  return reply.item.id;
}

/** Changes a backlog item, saying what else happened (a worktree taken away). */
export async function changeItem(
  id: number,
  change: ItemChange,
): Promise<boolean> {
  if (!conn) return false;
  try {
    const reply = await conn.request({ t: "change_item", id, change });
    if (reply.t === "item" && reply.note) toast(reply.note);
    return true;
  } catch (e) {
    toast((e as Error).message);
    return false;
  }
}

async function sendChange(id: number, change: ItemChange): Promise<string | null> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "change_item", id, change });
  return reply.t === "item" ? (reply.note ?? null) : null;
}

/** Sets several backlog items' status at once, under one Undo (the filed tray's
 * Keep and Drop, #252): `Kept 6` or `Dropped 6`. */
export function setStatuses(ids: number[], status: "open" | "dropped"): void {
  if (ids.length === 1) return setStatus(ids[0], status);
  const items = get().items;
  const changes: Change[] = ids.map((id) => {
    const item = items.find((i) => i.id === id);
    return { t: "item", id, patch: { status: status === "open" && item?.unaccepted ? "proposed" : status } };
  });
  hold(`${status === "open" ? "Kept" : "Dropped"} ${ids.length}`, changes, async () => {
    const notes = [];
    for (const id of ids) notes.push(await sendChange(id, { do: "status", status }));
    return notes.filter(Boolean).join(" · ") || null;
  });
}

/** Sets a backlog item's status at once, with Undo; the daemon hears when that's gone. */
export function setStatus(id: number, status: Item["status"]): void {
  // As shown: a change still held counts, such as a ship before this reopen.
  const item = get().items.find((i) => i.id === id);
  // Reopening one shipped or dropped before it was accepted proposes it again.
  const shown = status === "open" && item?.unaccepted ? "proposed" : status;
  hold(statusDone(item?.status, status, item?.number ?? id), { t: "item", id, patch: { status: shown } }, () =>
    sendChange(id, { do: "status", status }),
  );
}

/** Edits a backlog item at once, with Undo; the daemon hears when that's gone.
 * Refused, the toast offers the editor again with the edit in it. */
export function editItem(id: number, change: Extract<ItemChange, { do: "edit" }>): void {
  const retry = {
    label: "Edit again",
    run: () => {
      failedEdits.set(id, change);
      location.hash = `#/backlog/${id}`;
    },
  };
  hold(`Saved #${itemNo(get().items, id)}`, { t: "item", id, patch: editPatch(change) }, () => sendChange(id, change), retry);
}

/** Edits the daemon refused, by item, for the editor to open with again. */
const failedEdits = new Map<number, Extract<ItemChange, { do: "edit" }>>();

/** The refused edit of item `id` to start the editor from, once; null without one. */
export function takeFailedEdit(id: number): Extract<ItemChange, { do: "edit" }> | null {
  const change = failedEdits.get(id) ?? null;
  failedEdits.delete(id);
  return change;
}

/** Deletes a backlog item at once, with Undo; the daemon hears when that's gone. */
export function removeItem(id: number): void {
  hold(`Deleted #${itemNo(get().items, id)}`, { t: "item_gone", id }, async () => {
    if (!conn) throw new Error("not connected");
    const reply = await conn.request({ t: "delete_item", id });
    return reply.t === "item" ? (reply.note ?? null) : null;
  });
}

/** Leaves a handoff note on a backlog item. */
export async function noteItem(id: number, text: string): Promise<boolean> {
  if (!conn) return false;
  try {
    await conn.request({ t: "note_item", id, text });
    return true;
  } catch (e) {
    toast((e as Error).message);
    return false;
  }
}

/** The ids of the open items whose branch is merged, as the daemon last fetched (it
 * doesn't fetch, nor does `valk web`); with none, says why and returns none. */
export async function checkMerged(
  shown: (id: number) => boolean = () => true,
): Promise<number[]> {
  if (!conn) return [];
  try {
    const reply = await conn.request({
      t: "ship_merged",
      dry_run: true,
      only: null,
    });
    if (reply.t !== "shipped") return [];
    // Only the items the board shows, so a filtered board never ships another repository's.
    const shipped = reply.shipped.filter((s) => shown(s.id));
    const kept = reply.kept.filter((k) => shown(k.id));
    toast(
      shipped.length ? wouldShip(shipped, kept) : shippedSummary(shipped, kept),
    );
    return shipped.map((s) => s.id);
  } catch (e) {
    toast((e as Error).message);
    return [];
  }
}

/** Ships the items `checkMerged` found, those still merged, toasting what happened. */
export async function shipMerged(only: number[]): Promise<void> {
  if (!conn) return;
  try {
    const reply = await conn.request({
      t: "ship_merged",
      dry_run: false,
      only,
    });
    if (reply.t === "shipped") toast(shippedSummary(reply.shipped, reply.kept));
  } catch (e) {
    toast((e as Error).message);
  }
}

/** Deletes every worktree's idle cargo build output (#300), toasting what it freed. */
export async function freeDisk(): Promise<void> {
  if (!conn) return;
  try {
    const reply = await conn.request({ t: "free_disk" });
    if (reply.t === "text") toast(reply.text);
  } catch (e) {
    toast((e as Error).message);
  }
}

/** Starts or leaves the ready item the supervisor offers. Started, the new session's id. */
export async function answerLaunch(offer: LaunchOffer, launch: boolean): Promise<SessionId | null> {
  if (!conn) return null;
  try {
    const reply = await conn.request({ t: "answer_launch", project: offer.project, id: offer.id, launch });
    if (reply.t === "session") {
      reportColors(reply.info.id);
      return reply.info.id;
    }
    if (reply.t === "text") toast(reply.text);
    return null;
  } catch (e) {
    toast((e as Error).message);
    return null;
  }
}

/** Takes a step on a flagged session (ADR-0019 §2): the one offered, its alternative, or leave. */
export async function takeStep(
  session: SessionId,
  seq: number,
  action: StepAction,
): Promise<boolean> {
  if (!conn) return false;
  try {
    const reply = await conn.request({ t: "take_step", session, seq, action });
    if (reply.t === "step_taken") toast(reply.said);
    return true;
  } catch (e) {
    toast((e as Error).message);
    return false;
  }
}

/** What a backlog item's checkout holds now; null without one. */
export async function itemFacts(id: number): Promise<ItemFacts | null> {
  if (!conn) return null;
  const reply = await conn.request({ t: "item_facts", id });
  return reply.t === "item_facts" ? reply.facts : null;
}

/** Starts an agent on a backlog item, in a worktree of its own; the session's id.
 * `model` absent keeps the item's last one for that agent, `""` is the agent's own. */
export async function launchItem(
  id: number,
  agent: "claude" | "codex",
  worktree = true,
  model?: string,
): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({
    t: "launch_item",
    id,
    agent,
    worktree,
    size: SPAWN_SIZE,
    env: [],
    ...(model === undefined ? {} : { model }),
  });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  reportColors(reply.info.id);
  return reply.info.id;
}

/** Types a message into a session that may not be the open one. */
export function sendTo(session: SessionId, text: string): boolean {
  if (!text.trim()) return false;
  const modes = get().openId === session ? get().screen?.modes : undefined;
  return message(session, text, modes);
}

/** Answers several prompts, one after another, each with its own choice. */
export async function answerAll(
  answers: { session: SessionId; choices: Choice[]; choice: Choice }[],
): Promise<void> {
  // Every card shows sent at once; the keys still go one prompt at a time.
  // One answered on its own says so itself. Ones already on their way from
  // another tap aren't this batch's to count.
  const started = answers.filter((a) => startAnswer(a.session, a.choices, a.choice));
  const quiet = started.length > 1;
  const missed: Missed[] = [];
  for (const a of started) {
    const seq = statusSeq(a.session);
    if (!(await sendAnswer(a.session, a.choices, a.choice, quiet))) missed.push({ ...a, seq });
  }
  if (!quiet) return;
  toast(
    batchSummary(started.length - missed.length, started.length),
    missed.length ? { label: "Retry", run: () => void retryAll(missed) } : undefined,
  );
}

/** An Allow all answer that didn't go through, with the status `seq` it was sent at. */
type Missed = { session: SessionId; choices: Choice[]; choice: Choice; seq: number | null };

/** Tries again the ones still showing the prompt they missed. One that moved
 * on isn't sent the old answer's keys: another prompt, even with the same
 * choices, or the keys landed after all and only the confirm was lost. */
function retryAll(missed: Missed[]): void {
  const still = missed.filter((a) => {
    const now = get().prompts[a.session]?.choices;
    return (
      now !== undefined &&
      choicesKey(now) === choicesKey(a.choices) &&
      statusSeq(a.session) === a.seq
    );
  });
  if (!still.length) return toast("Those prompts have moved on");
  void answerAll(still);
}
