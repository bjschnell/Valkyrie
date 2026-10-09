// App state: the connection, the session list and queue as the daemon pushes them,
// the open session's screen, and the prompts waiting in the inbox.

import { create } from "zustand";
import { Conn, type Status } from "./net/conn";
import type { Decision, Modes, QueueItem, ReviewAction, ServerMsg, SessionId, SessionInfo } from "./proto";
import { applyChat, type Chat } from "./lib/chat";
import { findChoices, promptContext, type Choice } from "./lib/choices";
import { keyBytes, messageBytes, type Key } from "./lib/keys";
import { apply, screenText, type Screen } from "./lib/screen";
import {
  clearNotifications,
  disablePush,
  enablePush,
  pushState,
  setBadge,
  testPush,
  type PushState,
} from "./lib/push";

const TOKEN_KEY = "valk.token";
/** The list has no push of its own; the TUI polls it about as often. */
const LIST_EVERY_MS = 2_000;
/** Between keys of one answer, so the program reads them as separate presses. */
const KEY_GAP_MS = 70;
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
  sessions: SessionInfo[];
  queue: QueueItem[];
  /** Decisions agents proposed, waiting on the human (ADR-0007). */
  proposals: Decision[];
  /** The session on screen, its screen, and its exit once it has one. */
  openId: SessionId | null;
  screen: Screen | null;
  exited: { code: number | null } | null;
  /** The open session's agent conversation, from its transcript; `null` until it loads. */
  chat: Chat | null;
  /** Choices on the screens of queued sessions, by session, for the inbox. */
  prompts: Record<SessionId, Prompt>;
  toast: { text: string; at: number } | null;
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
  sessions: [],
  queue: [],
  proposals: [],
  openId: null,
  screen: null,
  exited: null,
  chat: null,
  prompts: {},
  toast: null,
  now: Date.now(),
  push: "unknown",
  pushBusy: false,
  canInstall: false,
}));

const set = useApp.setState;
const get = useApp.getState;
let conn: Conn | null = null;
let listTimer: number | null = null;

export function toast(text: string): void {
  set({ toast: { text, at: Date.now() } });
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

export function connect(): void {
  const token = get().token;
  if (!token || conn) return;
  conn = new Conn({
    token,
    onStatus: (status) => set({ status }),
    onPush,
    onReady: () => {
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
      set({ queue: msg.items });
      setBadge(msg.items.length);
      void refreshPrompts(msg.items);
      void refresh();
      break;
    case "proposals":
      set({ proposals: msg.items });
      break;
    case "screen":
      if (msg.session === get().openId) set({ screen: apply(get().screen, msg.update) });
      break;
    case "chat":
      if (msg.session === get().openId) set({ chat: applyChat(get().chat, msg) });
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

/** Tells the server whether this app is on screen: notifications wait while it is. */
function reportVisible(): void {
  const visible = document.visibilityState === "visible";
  conn?.send({ t: "visible", visible });
  if (visible) void clearNotifications();
}

export async function checkPush(): Promise<void> {
  const token = get().token;
  if (token) set({ push: await pushState(token).catch(() => "unsupported" as const) });
}

export async function turnOnPush(): Promise<void> {
  const token = get().token;
  if (!token || get().pushBusy) return;
  set({ pushBusy: true });
  try {
    const on = await enablePush(token);
    set({ push: on ? "on" : Notification.permission === "denied" ? "denied" : "off" });
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
    if (reply?.t === "sessions") set({ sessions: reply.sessions });
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
      prompts[item.session] = { seq: item.status.seq, choices: findChoices(lines), context: promptContext(lines) };
    } catch {
      /* shown without buttons */
    }
  }
  set({ prompts });
}

export async function open(id: SessionId): Promise<void> {
  if (get().openId === id) return;
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
  set({ openId: null, screen: null, exited: null, chat: null });
  conn?.send({ t: "chat", session: null });
  await conn?.request({ t: "detach" }).catch(() => {});
}

function input(session: SessionId, data: number[]): boolean {
  const ok = conn?.send({ t: "input", session, data }) ?? false;
  if (!ok) toast("not connected; nothing was sent");
  return ok;
}

export function sendKey(key: Key): void {
  const id = get().openId;
  if (id !== null) input(id, keyBytes(key, get().screen?.modes));
}

/** Types a message, then presses Enter once the program has read it as typing. */
function message(session: SessionId, text: string, modes?: Modes): boolean {
  if (!input(session, messageBytes(text, modes))) return false;
  setTimeout(() => input(session, keyBytes("enter", modes)), ENTER_GAP_MS);
  return true;
}

export function sendMessage(text: string): boolean {
  const id = get().openId;
  if (id === null || !text.trim()) return false;
  return message(id, text, get().screen?.modes);
}

/** Picks a choice: moves the program's cursor to it, then Enter. */
export async function answer(session: SessionId, choices: Choice[], choice: Choice): Promise<void> {
  const from = choices.findIndex((c) => c.selected);
  const to = choices.indexOf(choice);
  const moves = choice.moves ?? (from >= 0 ? to - from : null);
  const modes = get().openId === session ? get().screen?.modes : undefined;
  const keys: Key[] =
    moves === null ? [] : [...Array(Math.abs(moves)).fill(moves < 0 ? "up" : "down"), "enter"];
  navigator.vibrate?.(10);
  if (moves === null) {
    // No cursor to move: the number picks it.
    input(session, [...new TextEncoder().encode(String(choice.n ?? to + 1))]);
  } else {
    for (const [i, key] of keys.entries()) {
      if (i) await new Promise((r) => setTimeout(r, KEY_GAP_MS));
      if (!input(session, keyBytes(key, modes))) return;
    }
  }
  // Answered: drop the buttons until the screen says otherwise.
  const prompts = { ...get().prompts };
  delete prompts[session];
  set({ prompts });
  toast(`${choice.label.slice(0, 40)} ✓`);
}

export async function markSeen(item: QueueItem): Promise<void> {
  await conn?.request({ t: "mark_seen", session: item.session, seq: item.status.seq }).catch(() => {});
}

export async function rename(id: SessionId, name: string | null): Promise<void> {
  await conn?.request({ t: "rename", session: id, name }).catch((e) => toast(String(e.message)));
  void refresh();
}

export async function kill(id: SessionId): Promise<void> {
  await conn?.request({ t: "kill", session: id }).catch((e) => toast(String(e.message)));
  void refresh();
}

const REVIEWED: Record<ReviewAction["a"], string> = {
  accept: "Accepted",
  reject: "Rejected",
  retire: "Retired",
  edit: "Saved",
  revise: "Accepted",
};

/** Accepts, rejects or rewords a proposed decision; the daemon pushes the new list. */
export async function review(d: Decision, action: ReviewAction): Promise<boolean> {
  try {
    if (!conn) throw new Error("not connected");
    await conn.request({ t: "review", project: d.project, id: d.id, action });
    navigator.vibrate?.(10);
    toast(`${REVIEWED[action.a]} #${d.id} ✓`);
    return true;
  } catch (e) {
    toast(`Couldn't: ${(e as Error).message}`);
    return false;
  }
}

/** The choices on the open session's screen right now. */
export function openChoices(screen: Screen | null): Choice[] {
  return screen ? findChoices(screenText(screen)) : [];
}

/** The size a new session starts at; a TUI that attaches later resizes it. */
const SPAWN_SIZE = { cols: 120, rows: 40 };

/** Starts a session (DESIGN §8.8) and returns its id. */
export async function spawn(command: string[], cwd: string, name: string | null): Promise<SessionId> {
  if (!conn) throw new Error("not connected");
  const reply = await conn.request({ t: "spawn", spec: { command, cwd, name, size: SPAWN_SIZE, env: [] } });
  if (reply.t !== "session") throw new Error("unexpected reply");
  void refresh();
  return reply.info.id;
}

/** Types a message into a session that may not be the open one. */
export function sendTo(session: SessionId, text: string): boolean {
  if (!text.trim()) return false;
  const modes = get().openId === session ? get().screen?.modes : undefined;
  return message(session, text, modes);
}

/** Answers several prompts, one after another, each with its own choice. */
export async function answerAll(answers: { session: SessionId; choices: Choice[]; choice: Choice }[]): Promise<void> {
  for (const a of answers) await answer(a.session, a.choices, a.choice);
  if (answers.length > 1) toast(`Answered ${answers.length} prompts ✓`);
}
