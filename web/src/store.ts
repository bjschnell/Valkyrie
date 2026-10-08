// App state: the connection, the session list and queue as the daemon pushes them,
// the open session's screen, and the prompts waiting in the inbox.

import { create } from "zustand";
import { Conn, type Status } from "./net/conn";
import type { QueueItem, ServerMsg, SessionId, SessionInfo } from "./proto";
import { findChoices, type Choice } from "./lib/choices";
import { keyBytes, messageBytes, type Key } from "./lib/keys";
import { apply, screenText, type Screen } from "./lib/screen";

const TOKEN_KEY = "valk.token";
/** The list has no push of its own; the TUI polls it about as often. */
const LIST_EVERY_MS = 2_000;
/** Between keys of one answer, so the program reads them as separate presses. */
const KEY_GAP_MS = 70;

export interface Prompt {
  seq: number;
  choices: Choice[];
}

interface State {
  token: string | null;
  status: Status;
  sessions: SessionInfo[];
  queue: QueueItem[];
  /** The session on screen, its screen, and its exit once it has one. */
  openId: SessionId | null;
  screen: Screen | null;
  exited: { code: number | null } | null;
  /** Choices on the screens of queued sessions, by session, for the inbox. */
  prompts: Record<SessionId, Prompt>;
  toast: { text: string; at: number } | null;
  now: number;
}

export const useApp = create<State>(() => ({
  token: localStorage.getItem(TOKEN_KEY),
  status: "connecting",
  sessions: [],
  queue: [],
  openId: null,
  screen: null,
  exited: null,
  prompts: {},
  toast: null,
  now: Date.now(),
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
      void conn?.request({ t: "watch_queue" }).catch(() => {});
      void refresh();
      const open = get().openId;
      if (open !== null) void attach(open);
    },
  });
  conn.start();
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
      void refreshPrompts(msg.items);
      void refresh();
      break;
    case "screen":
      if (msg.session === get().openId) set({ screen: apply(get().screen, msg.update) });
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
      prompts[item.session] = { seq: item.status.seq, choices: findChoices(reply.text.split("\n")) };
    } catch {
      /* shown without buttons */
    }
  }
  set({ prompts });
}

export async function open(id: SessionId): Promise<void> {
  if (get().openId === id) return;
  set({ openId: id, screen: null, exited: null });
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
  set({ openId: null, screen: null, exited: null });
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

export function sendMessage(text: string): boolean {
  const id = get().openId;
  if (id === null || !text.trim()) return false;
  return input(id, messageBytes(text, get().screen?.modes));
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

/** The choices on the open session's screen right now. */
export function openChoices(screen: Screen | null): Choice[] {
  return screen ? findChoices(screenText(screen)) : [];
}
