// The daemon's wire protocol (crates/valkyrie-proto), as the web app sees it: the
// `valk web` server passes these JSON messages through unchanged.

export const PROTOCOL = 10;

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
  /** The agent's own transcript, when known: the Chat view reads it. */
  chat?: string;
}

export interface QueueItem {
  session: SessionId;
  name: string;
  cwd: string;
  status: AgentStatus;
}

/** A project decision (ADR-0007). */
export type DecisionKind = "decision" | "constraint" | "pattern" | "gotcha" | "fix";
export const DECISION_KINDS: DecisionKind[] = ["decision", "constraint", "pattern", "gotcha", "fix"];
export type DecisionStatus = "proposed" | "active" | "rejected" | "superseded" | "retired";

export interface Decision {
  id: number;
  /** The project's root directory. */
  project: string;
  title: string;
  body: string;
  kind: DecisionKind;
  status: DecisionStatus;
  /** Seconds since the epoch. */
  created: number;
  updated: number;
  supersedes?: number;
  superseded_by?: number;
  provenance: {
    /** `human`, or the agent that proposed it. */
    by: string;
    session?: string;
    conversation?: string;
    commit?: string;
    cwd?: string;
  };
}

export type ReviewAction =
  | { a: "accept" }
  | { a: "reject" }
  | { a: "retire" }
  | { a: "edit"; title: string; body: string; kind: DecisionKind };

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
  | { t: "spawn"; spec: SpawnSpec }
  | { t: "review"; project: string; id: number; action: ReviewAction }
  | { t: "decisions"; cwd: string | null };

/** Fire-and-forget messages. */
export type Notice =
  | { t: "input"; session: SessionId; data: number[] }
  /** To `valk web` itself, not the daemon: whether this app is on screen, which
   * holds back notifications. */
  | { t: "visible"; visible: boolean }
  /** To `valk web`: follow this session's agent transcript (`null`: stop). */
  | { t: "chat"; session: SessionId | null };

export type Reply =
  | { t: "done" }
  | { t: "hello"; protocol: number; generation: number; boot: number }
  | { t: "session"; info: SessionInfo }
  | { t: "sessions"; sessions: SessionInfo[] }
  | { t: "text"; text: string }
  | { t: "decision"; decision: Decision }
  | { t: "decisions"; decisions: Decision[] };

export type ServerMsg =
  | { t: "ok"; req: number; reply: Reply }
  | { t: "err"; req: number; message: string }
  | { t: "screen"; session: SessionId; update: ScreenUpdate }
  | { t: "exited"; session: SessionId; code: number | null }
  | { t: "queue"; items: QueueItem[] }
  /** Every project's decisions waiting on review, oldest first. */
  | { t: "proposals"; items: Decision[] }
  | { t: "clipboard"; session: SessionId; text: string }
  | { t: "graphics"; session: SessionId }
  | ChatUpdate;

/** One entry in the Chat view (crates/valkyrie-web/src/chat.rs). */
export type ChatItem =
  | { k: "user"; id: string; text: string; at?: string }
  | { k: "say"; id: string; text: string; at?: string }
  | { k: "tool"; id: string; tool: string; summary: string; status: ToolStatus; detail: ToolDetail }
  | { k: "mark"; id: string; text: string; at?: string };
export type ToolStatus = "running" | "ok" | "error" | "denied";
export interface ToolDetail {
  file?: string;
  command?: string;
  diff?: string;
  content?: string;
  result?: string;
}
/** From `valk web`: a reset replaces the chat; otherwise items are added, or
 * replace the item with their id (a tool call that finished). */
export interface ChatUpdate {
  t: "chat";
  session: SessionId;
  items: ChatItem[];
  reset?: boolean;
  /** Older items exist that weren't loaded. */
  more?: boolean;
  /** No transcript known for this session. */
  missing?: boolean;
  /** The session is gone. */
  gone?: boolean;
}

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
