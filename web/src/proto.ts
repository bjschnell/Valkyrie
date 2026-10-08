// The daemon's wire protocol (crates/valkyrie-proto), as the web app sees it: the
// `valk web` server passes these JSON messages through unchanged.

export const PROTOCOL = 8;

export type SessionId = number;
export type Size = { cols: number; rows: number };

export type AgentState =
  | "working"
  | "needs_input"
  | "blocked"
  | "review_ready"
  | "idle"
  | "interrupted"
  | "stale"
  | "exited";

export type AskKind = "permission" | "question" | "input" | "screen" | "bell";

export interface AgentStatus {
  agent: string;
  state: AgentState;
  ask: AskKind | null;
  summary: string | null;
  since_ms: number;
  seq: number;
  seen: boolean;
  hooked: boolean;
}

export interface SessionInfo {
  id: SessionId;
  name: string;
  command: string[];
  cwd: string;
  pid: number | null;
  created_unix: number;
  title: string | null;
  clients: number;
  exited: number | null;
  status: AgentStatus;
}

export interface QueueItem {
  session: SessionId;
  name: string;
  cwd: string;
  status: AgentStatus;
}

export type Color = "default" | { indexed: number } | { rgb: [number, number, number] };

export interface Style {
  fg?: Color;
  bg?: Color;
  flags?: number;
}

export const BOLD = 1;
export const ITALIC = 1 << 1;
export const UNDERLINE = 1 << 2;
export const INVERSE = 1 << 3;
export const DIM = 1 << 4;
export const HIDDEN = 1 << 5;
export const STRIKEOUT = 1 << 6;

export interface Span {
  x: number;
  text: string;
  style?: Style;
}

export interface Row {
  y: number;
  spans: Span[];
  wrapped?: boolean;
}

export interface Modes {
  app_cursor: boolean;
  bracketed_paste: boolean;
  alt_screen: boolean;
}

export interface ScreenUpdate {
  full: boolean;
  size: Size;
  rows: Row[];
  cursor: { x: number; y: number; visible: boolean };
  modes: Modes;
  title: string | null;
}

export interface SpawnSpec {
  command: string[];
  cwd: string | null;
  name: string | null;
  size: Size;
  env: [string, string | null][];
}

/** Requests, without the `req` id the client adds. */
export type Request =
  | { t: "hello" }
  | { t: "list" }
  | { t: "watch_queue" }
  | { t: "attach"; session: SessionId; size: Size | null }
  | { t: "detach" }
  | { t: "dump"; session: SessionId }
  | { t: "kill"; session: SessionId }
  | { t: "mark_seen"; session: SessionId; seq: number }
  | { t: "rename"; session: SessionId; name: string | null }
  | { t: "move"; session: SessionId; to: number }
  | { t: "spawn"; spec: SpawnSpec };

/** Fire-and-forget messages. */
export type Notice = { t: "input"; session: SessionId; data: number[] };

export type Reply =
  | { t: "done" }
  | { t: "hello"; protocol: number; generation: number; boot: number }
  | { t: "session"; info: SessionInfo }
  | { t: "sessions"; sessions: SessionInfo[] }
  | { t: "text"; text: string };

export type ServerMsg =
  | { t: "ok"; req: number; reply: Reply }
  | { t: "err"; req: number; message: string }
  | { t: "screen"; session: SessionId; update: ScreenUpdate }
  | { t: "exited"; session: SessionId; code: number | null }
  | { t: "queue"; items: QueueItem[] }
  | { t: "clipboard"; session: SessionId; text: string }
  | { t: "graphics"; session: SessionId };

/** What a session runs: its agent, else its program (`fish`). */
export function program(s: SessionInfo): string {
  const agent = s.status.agent;
  if (agent && agent !== "generic" && agent !== "unknown") return agent;
  const first = s.command[0] ?? "?";
  return first.split("/").pop() || first;
}

export const STATE_LABEL: Record<AgentState, string> = {
  working: "working",
  needs_input: "needs input",
  blocked: "blocked",
  review_ready: "done",
  idle: "idle",
  interrupted: "interrupted?",
  stale: "stale",
  exited: "exited",
};
