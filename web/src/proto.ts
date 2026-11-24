// The daemon's wire protocol (crates/valkyrie-proto), as the web app sees it: the
// `valk web` server passes these JSON messages through unchanged. The types come
// from proto.gen.ts, generated from the Rust ones; this file adds what only the
// web app and `valk web` speak, and the constants the UI keeps beside them.

import type {
  AgentState,
  ClientMsg,
  DecisionKind,
  ItemKind,
  Priority,
  Reply as DaemonReply,
  ServerMsg as DaemonServerMsg,
  SessionInfo,
  StepAction,
} from "./proto.gen";

export type * from "./proto.gen";

export type SessionId = number;

/** Requests, without the `req` id the client adds. */
export type Request = ClientMsg extends infer M
  ? M extends { req: number }
    ? Omit<M, "req">
    : never
  : never;

/** Fire-and-forget messages: the daemon's, and those only `valk web` reads. */
export type Notice =
  | Exclude<ClientMsg, { req: number }>
  /** To `valk web` itself, not the daemon: whether this app is on screen, which
   * holds back notifications. */
  | { t: "visible"; visible: boolean }
  /** To `valk web`: follow this session's agent transcript (`null`: stop). */
  | { t: "chat"; session: SessionId | null };

/** The daemon's replies; `valk web` adds two fields to `hello`: `web`, the build
 * of the app it serves (see lib/build.ts), and `app_protocol`, the protocol that
 * build speaks (see lib/protocol.ts). */
export type Reply =
  | Exclude<DaemonReply, { t: "hello" }>
  | (Extract<DaemonReply, { t: "hello" }> & { web?: string; app_protocol?: number });

export type ServerMsg =
  | Exclude<DaemonServerMsg, { t: "ok" }>
  | { t: "ok"; req: number; reply: Reply }
  | ChatUpdate;

/** What the supervisor can do for a flagged session (ADR-0019 §2). */
export const STEP_LABEL: Record<StepAction, string> = {
  "merge-base": "Ask its agent to merge the base in",
  "ci-fix": "Ask its agent to fix CI",
  rerun: "Rerun failed jobs",
  leave: "Leave it",
  launch: "Start an agent on it",
};

export const DECISION_KINDS: DecisionKind[] = [
  "decision",
  "constraint",
  "pattern",
  "gotcha",
  "fix",
  "skill",
  "policy",
];

/** `Style.flags` bits. */
export const BOLD = 1;
export const ITALIC = 1 << 1;
export const UNDERLINE = 1 << 2;
export const INVERSE = 1 << 3;
export const DIM = 1 << 4;
export const HIDDEN = 1 << 5;
export const STRIKEOUT = 1 << 6;

export const ITEM_KINDS: ItemKind[] = ["bug", "feature", "chore", "spike", "debt"];
export const PRIORITIES: Priority[] = ["p0", "p1", "p2", "p3"];

/** One entry in the Chat view (crates/valkyrie-web/src/chat.rs). */
export type ChatItem =
  | { k: "user"; id: string; text: string; at?: string }
  | { k: "say"; id: string; text: string; at?: string }
  | {
      k: "tool";
      id: string;
      tool: string;
      summary: string;
      status: ToolStatus;
      detail: ToolDetail;
    }
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
