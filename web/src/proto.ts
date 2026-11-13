// The daemon's wire protocol (crates/valkyrie-proto), as the web app sees it: the
// `valk web` server passes these JSON messages through unchanged.

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
  /** `name` is what the agent says it works on, not one the user gave: shown set apart. */
  auto_name?: boolean;
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
  /** Another live agent in the repository changed the same files (protocol 31). */
  overlap?: string;
  /** A short status it published with `valk status`, e.g. "tests 41/50" (protocol 39). */
  report?: string;
}

export interface QueueItem {
  session: SessionId;
  name: string;
  auto_name?: boolean;
  cwd: string;
  status: AgentStatus;
  /** The supervisor's flag on this session's PR, and the step it offers (protocol 25). */
  flag?: Flag;
  /** The session's published status (`SessionInfo.report`). */
  report?: string;
}

/** What the supervisor can do for a flagged session (ADR-0019 §2). */
export type StepAction = "merge-base" | "ci-fix" | "rerun" | "leave" | "launch";

export const STEP_LABEL: Record<StepAction, string> = {
  "merge-base": "Merge the base in",
  "ci-fix": "Fix CI",
  rerun: "Rerun failed jobs",
  leave: "Leave it",
  launch: "Start it",
};

export interface Step {
  action: StepAction;
  /** Why it was picked, in a line. */
  why: string;
  /** The other action that would fit. */
  alt?: StepAction;
}

/** A watcher's flag on an idle session: its reason and the step offered. */
export interface Flag {
  reason: string;
  pr: string;
  /** The transition it raised; `take_step` names it. */
  seq: number;
  /** Absent while the supervisor works out its step (`working`), and once one was taken or left. */
  step?: Step;
  working?: boolean;
}

/** A project decision (ADR-0007). */
/** How the away digest is grouped (backlog #105). */
export type DigestGroup = "repo" | "day";
/** Where a session's work ended up, most urgent first. */
export type Outcome = "stuck" | "abandoned" | "waiting" | "working" | "shipped" | "done";

/** One agent session in the away digest. */
export interface DigestEntry {
  /** Its event log's name: stable across daemon restarts. */
  key: string;
  /** The live session, while it runs. */
  session?: SessionId;
  repo: string;
  cwd?: string;
  agent: string;
  /** What it was about, one line. */
  what: string;
  item?: number;
  branch?: string;
  pr?: string;
  outcome: Outcome;
  /** Why that outcome, one line. */
  detail: string;
  /** The agent's own last line. */
  summary?: string;
  prompts: number;
  first_ms: number;
  last_ms: number;
  ended_ms?: number;
}

export interface Digest {
  since_ms: number;
  until_ms: number;
  /** `since_ms` is when you were last gone, not a time you picked. */
  away: boolean;
  by: DigestGroup;
  entries: DigestEntry[];
  /** The same, as markdown to paste. */
  markdown: string;
}

/** An agent session that has ended, as the history lists it (backlog #150). */
export interface PastSession extends DigestEntry {
  started_ms: number;
  /** Why it can't be resumed; absent: it can. */
  cannot?: string;
  /** Its directory is gone: resuming adds its worktree back on this branch. */
  regrow?: string;
}

/** Where in a session a search hit was found (backlog #106): your prompt, the
 * agent's closing reply, a file it wrote, or what it was about. */
export type SearchField = "you" | "agent" | "file" | "about";

/** One session whose history holds every word, at its best line. */
export interface SearchHit {
  /** Its event log's name: stable across daemon restarts. */
  key: string;
  /** The live session, while it runs: open it. */
  session?: SessionId;
  repo: string;
  cwd?: string;
  /** `cwd` no longer exists (a removed worktree): a resume can't start there. */
  gone?: boolean;
  agent: string;
  /** What it was about, one line. */
  what: string;
  item?: number;
  branch?: string;
  pr?: string;
  field: SearchField;
  /** The best line, clipped around the words. */
  snippet: string;
  at_ms: number;
  /** Lines in the session holding every word. */
  lines: number;
  last_ms: number;
  ended_ms?: number;
  /** A closed session: the command that resumes its conversation, run in `cwd`. */
  resume?: string[];
}

export interface SearchResults {
  query: string;
  since_ms: number;
  until_ms: number;
  /** Newest first, at most the limit. */
  hits: SearchHit[];
  /** Sessions that matched, before the limit. */
  total: number;
}

export type DocKind = "plan" | "research" | "report" | "note";

/** A doc in the docs folder (ADR-0021). `key` is what it's opened by: its
 * `valk-id`, or a handle the daemon made for one of the user's own; never a path. */
export interface Doc {
  id: number;
  kind: DocKind;
  title: string;
  /** Relative to the docs folder, '/'-separated. */
  path: string;
  key?: string;
  /** An agent wrote or edited it since the Docs list was last opened. */
  unread?: boolean;
  by: string;
  created: string;
  session?: string;
  commit?: string;
  item?: number;
  repo?: string;
  edited?: string[];
  hash?: string;
  /** The whole file, front matter and all; only when opened. */
  text?: string;
}

/** One of a doc's past versions (ADR-0021 §4): the file as it was before an edit replaced it. */
export interface DocVersion {
  /** What `doc_history` and `restore_doc` name it by. */
  version: string;
  /** When it was replaced, in seconds. */
  at: number;
  /** Who wrote it. */
  by: string;
  size: number;
}

/** A doc the user deleted, kept so it can be put back. */
export interface DeletedDoc {
  /** What `undelete_doc` takes. */
  token: string;
  /** When it was deleted, in seconds. */
  at: number;
  path: string;
  title: string;
}

/** Where a comment on a doc stands (ADR-0021 §6). */
export type CommentState = "open" | "sent" | "answered" | "resolved";

/** A comment the user left on a doc, or on one block of it. */
export interface DocComment {
  id: number;
  /** When it was left, in seconds. */
  at: number;
  /** The block it's about, as it read then, on one line; none for the whole doc. */
  quote?: string;
  text: string;
  state: CommentState;
  /** The session it was told to. */
  sent_to?: string;
  changed: number;
}

export type DecisionKind =
  "decision" | "constraint" | "pattern" | "gotcha" | "fix" | "skill" | "policy";
export const DECISION_KINDS: DecisionKind[] = [
  "decision",
  "constraint",
  "pattern",
  "gotcha",
  "fix",
  "skill",
  "policy",
];
export type DecisionStatus =
  "proposed" | "active" | "rejected" | "superseded" | "retired";

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
  /** Its body goes in every session's block (ADR-0014 §1); only you set it. */
  pinned?: boolean;
  /** Path globs relative to the project root it applies to (ADR-0014 §2): agents
   * get its body when they first open a file in them. Absent: everywhere. */
  scope?: string[];
  /** A scope proposed for it once, waiting on you. */
  proposed_scope?: string[];
  /** Its scope is settled, so it isn't proposed one again. */
  scope_asked?: boolean;
  provenance: {
    /** `human`, or the agent that proposed it. */
    by: string;
    session?: string;
    conversation?: string;
    commit?: string;
    cwd?: string;
  };
  fresh?: {
    confirmed?: number;
    review_every?: number;
    anchors?: string[];
    /** Why an active decision may be out of date (DESIGN §6.4). */
    review?: string;
  };
}

export type ReviewAction =
  | { a: "accept" }
  | { a: "reject" }
  | { a: "retire" }
  /** An active decision flagged for review still holds. */
  | { a: "confirm" }
  /** Put an active decision's body in every session's block, or take it back to a title. */
  | { a: "pin" }
  | { a: "unpin" }
  /** `scope` absent leaves it as it is. */
  | {
      a: "edit";
      title: string;
      body: string;
      kind: DecisionKind;
      scope?: string[];
    }
  /** Reword a proposal and accept it; refused once it isn't a proposal any more. */
  | {
      a: "revise";
      title: string;
      body: string;
      kind: DecisionKind;
      scope?: string[];
    }
  /** Set where it applies, settling any proposed scope; empty: everywhere. */
  | { a: "scope"; scope: string[] }
  /** Propose a scope for an active decision that has none, once. */
  | { a: "propose_scope"; scope: string[] }
  /** Turn a proposed scope down; it isn't proposed again. */
  | { a: "decline_scope" };

export type Color =
  "default" | { indexed: number } | { rgb: [number, number, number] };

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
  /** 1004: the program wants to know when the terminal gains or loses the keyboard. */
  focus_events?: boolean;
  /** 1007: the wheel scrolls by sending arrow keys in the alternate screen. */
  alt_scroll?: boolean;
  /** The program asked for the mouse: clicks (1000), drags too (1002), every move (1003). */
  mouse_click?: boolean;
  mouse_drag?: boolean;
  mouse_motion?: boolean;
  /** How it wants reports written: 1006 (SGR) or 1005 (UTF-8); neither is the old X10 form. */
  mouse_sgr?: boolean;
  mouse_utf8?: boolean;
}

/** Where a `scrollback` page starts: lines above the live screen, or a line
 * counted from the oldest in history (which stays put while output arrives). */
export type ScrollAnchor = { up: number } | { from_top: number };

export interface ScreenUpdate {
  full: boolean;
  size: Size;
  rows: Row[];
  cursor: Cursor;
  modes: Modes;
  title: string | null;
}

export interface Cursor {
  x: number;
  y: number;
  visible: boolean;
  shape?: "block" | "underline" | "beam";
}

export interface SpawnSpec {
  command: string[];
  cwd: string | null;
  name: string | null;
  size: Size;
  env: [string, string | null][];
  /** A new git worktree to start in, on this branch; "" names it after the session. */
  worktree?: string | null;
}

/** A backlog item (DESIGN §8.12): written down for later, it can start an agent on itself. */
export type ItemKind = "bug" | "feature" | "chore";
export const ITEM_KINDS: ItemKind[] = ["bug", "feature", "chore"];
export type ItemStatus = "open" | "shipped" | "dropped" | "proposed";
/** How soon: p0 first. An item from an older daemon has none, which is p2. */
export type Priority = "p0" | "p1" | "p2" | "p3";
export const PRIORITIES: Priority[] = ["p0", "p1", "p2", "p3"];

export interface Item {
  id: number;
  title: string;
  body: string;
  kind: ItemKind;
  status: ItemStatus;
  priority?: Priority;
  /** Items to finish first: advisory, it sorts below the ready ones while one is open. */
  blocked_by?: number[];
  /** The repository it's for; absent until one is picked. */
  project?: string;
  attachments?: string[];
  /** `human`, or the agent that wrote it down. */
  by: string;
  /** Sessions started on it, oldest first; ids of the daemon whose `boot` this is. */
  sessions?: SessionId[];
  boot: number;
  /** Those of `sessions` that took it rather than being launched on it. */
  taken?: SessionId[];
  /** When each of `sessions` last took or was launched on it, as [session, seconds
   * since the epoch]: the item a session is on is the one it joined last. */
  joined?: [SessionId, number][];
  branch?: string;
  /** `branch` was set by `valk todo take`, not a launch. */
  branch_taken?: boolean;
  checkout?: string;
  /** The pull request's URL, as `valk todo take --pr` recorded it. */
  pr?: string;
  /** The daemon boot and session that wrote it down and took it (`valk todo add --take`). */
  filed_by?: [number, number];
  /** Shipped or dropped while still proposed: reopening makes it proposed again. */
  unaccepted?: boolean;
  pr_closed?: boolean;
  /** Handoffs left on it, oldest first. */
  notes?: ItemNote[];
  /** The agent and model its last launch ran; `model` absent for the agent's default. */
  launch?: { agent: string; model?: string };
  /** Seconds since the epoch. */
  created: number;
  updated: number;
}

export interface ItemNote {
  /** Seconds since the epoch. */
  at: number;
  /** `human`, or the agent that left it. */
  by: string;
  text: string;
  /** From an agent that took the item rather than was launched on it: never in a brief. */
  taken?: boolean;
  /** From an agent on other work, neither launched on it nor took it: never in a brief. */
  aside?: boolean;
  /** For an aside, the item that agent was on, when it was on one. */
  from?: number;
}

/** What an item's checkout holds, from git. */
export interface ItemFacts {
  branch: string;
  ahead: number;
  commits: string[];
  /** `git diff --shortstat`; empty when nothing changed. */
  diff: string;
  dirty: number;
}

/** An item `ship_merged` shipped: `against` is what its branch is merged into. */
export interface ShippedItem {
  id: number;
  title: string;
  /** The branch judged; `branch_taken` when a take, not a launch, set it. */
  branch: string;
  branch_taken?: boolean;
  /** An agent filed it and it isn't accepted yet: shipping it accepts it. */
  filed?: boolean;
  against: string;
  note?: string;
}

/** An open item with a branch that `ship_merged` kept, and why. */
export interface KeptItem {
  id: number;
  title: string;
  why: string;
}

export interface NewItem {
  title: string;
  body: string;
  kind: ItemKind;
  cwd: string | null;
  attachments: string[];
  priority?: Priority;
  blocked_by?: number[];
}

export type ItemChange =
  | {
      do: "edit";
      title?: string;
      body?: string;
      kind?: ItemKind;
      cwd?: string | null;
      priority?: Priority;
      /** The whole list, replacing the item's; empty clears it. */
      blocked_by?: number[];
    }
  | { do: "status"; status: ItemStatus }
  /** Puts a started item back in the backlog and stops its sessions (protocol 28). */
  | { do: "return" };

/** One item's place in a build order: why, and what accepting it changes
 * (`priority` and the whole `blocked_by` list; absent keeps what it has). */
export interface OrderStep {
  item: number;
  reason: string;
  priority?: Priority;
  blocked_by?: number[];
}

/** A build order for one repository's open items (ADR-0019 §2), waiting on the
 * user: accept all, some (`items`) or none. Never applied on its own. */
export interface OrderProposal {
  /** Changes when the repository's proposal is replaced. */
  id: number;
  project: string;
  /** Seconds since the epoch; an item edited after it keeps its edit. */
  made: number;
  /** What asked for it: `you asked`, `3+ items added within a day`, … */
  trigger: string;
  steps: OrderStep[];
}

/** A ready item the supervisor offers to start (ADR-0019 §2 `launch`, protocol 27):
 * open, nobody on it, its blockers finished (or waived by the model), with room
 * under the repository's `max_running`. One per repository at most. */
/** How much of the plan is used (protocol 30), as Claude Code's status line last
 * said it: only as new as the last Claude session to draw one. */
export interface Quota {
  /** When it was read, seconds since the epoch. */
  at: number;
  /** Shortest window first. */
  limits: QuotaLimit[];
}

export interface QuotaLimit {
  /** Claude Code's name: `five_hour`, `seven_day`, `seven_day_opus`, … */
  name: string;
  /** Share used, 0–100. */
  used: number;
  /** Seconds since the epoch. */
  resets_at?: number;
}

export interface LaunchOffer {
  /** Changes when the repository's offer is replaced. */
  id: number;
  project: string;
  item: number;
  title: string;
  /** The agent and model it starts on: its last launch, else claude's default. */
  agent: string;
  model?: string;
  /** Why this item, now, in a line. */
  why: string;
  /** Unfinished blockers the model judged it doesn't need first. */
  waived?: number[];
  /** Seconds since the epoch. */
  made: number;
}

/** What accepting a build order did: items changed, and those left as they were. */
export interface OrderApplied {
  changed: number[];
  skipped?: [number, string][];
}

/** Requests, without the `req` id the client adds. */
export type Request =
  | { t: "hello" }
  | { t: "list" }
  | { t: "watch_queue" }
  | { t: "attach"; session: SessionId; size: Size | null }
  | { t: "detach" }
  | { t: "dump"; session: SessionId }
  /** One screenful of history, then the screen; replies `scrollback`. */
  | { t: "scrollback"; session: SessionId; anchor: ScrollAnchor }
  | { t: "kill"; session: SessionId }
  | { t: "mark_seen"; session: SessionId; seq: number }
  | { t: "rename"; session: SessionId; name: string | null }
  | { t: "move"; session: SessionId; to: number }
  | { t: "spawn"; spec: SpawnSpec }
  | {
      t: "review";
      project: string;
      id: number;
      action: ReviewAction;
      seen: number;
    }
  | { t: "decisions"; cwd: string | null }
  /** Every doc, newest first; replies `docs`. */
  /** `count_only`: no docs, just the unread count (the Inbox's poll). */
  | { t: "docs"; count_only?: boolean }
  /** What every agent did since `since_ms`, or since you were last gone (protocol 37). */
  | { t: "digest"; since_ms?: number | null; by: DigestGroup }
  /** Agent sessions that have ended, newest first (protocol 41); replies `history`. */
  | { t: "history" }
  /** Starts a past session again on its own conversation; replies `session`. */
  | { t: "resume"; key: string; size: { cols: number; rows: number }; env: [string, string | null][] }
  /** Sessions whose history holds every word of `query` (backlog #106, protocol 40). */
  | { t: "search"; query: string; since_ms?: number | null; repo?: string | null; limit?: number | null }
  /** A doc by the key a list gave it, never by path; replies `doc`. Writes nothing. */
  | { t: "open_doc"; key: string }
  /** A doc's comments; replies `doc_comments`. */
  | { t: "doc_comments"; key: string }
  /** The user comments on a doc, or the block `quote` reads; replies `doc_comments`. */
  | { t: "comment_doc"; key: string; quote?: string; text: string }
  /** The user closes a comment; replies `doc_comments`. */
  | { t: "resolve_comment"; key: string; comment: number }
  /** The Docs list was opened: what agents wrote until now is read. */
  | { t: "docs_seen"; at: number }
  /** The user's save from the editor, if `base` is still on disk; replies `doc_saved`. */
  | { t: "save_doc"; key: string; base: string; body: string }
  /** A doc's past versions, and `version`'s text when given; replies `doc_history`. */
  | { t: "doc_history"; key: string; version?: string }
  /** Puts a past version back, under the same rule as a save; replies `doc_saved`. */
  | { t: "restore_doc"; key: string; base: string; version: string }
  /** Deletes a doc, keeping a copy; replies `doc_deleted`, or `doc_saved` with `conflict`. */
  | { t: "delete_doc"; key: string; base: string }
  /** Puts a deleted doc back; replies `doc`. */
  | { t: "undelete_doc"; token: string }
  /** Deleted docs that can still be put back, newest first; replies `deleted_docs`. */
  | { t: "deleted_docs" }
  | { t: "add_item"; item: NewItem }
  | { t: "items" }
  | { t: "change_item"; id: number; change: ItemChange }
  | { t: "delete_item"; id: number }
  | { t: "note_item"; id: number; text: string }
  | { t: "item_facts"; id: number }
  /** Asks the model for a build order of `cwd`'s repository (protocol 24). */
  | { t: "plan_order"; cwd: string }
  /** The user only: `items` accepts only those items' changes, absent all of them. */
  | {
      t: "answer_order";
      project: string;
      id: number;
      accept: boolean;
      items?: number[];
    }
  | { t: "take_step"; session: SessionId; seq: number; action: StepAction }
  /** The user only: starts (`launch`) or leaves the repository's ready item;
   * replies `session` when started, `text` when left (protocol 27). */
  | { t: "answer_launch"; project: string; id: number; launch: boolean }
  /** Ships every open item whose branch is merged, as last fetched; `dry_run` only
   * says what would ship, `only` ships just those (each checked again). */
  | { t: "ship_merged"; dry_run: boolean; only: number[] | null }
  | {
      t: "launch_item";
      id: number;
      agent: "claude" | "codex";
      worktree: boolean;
      size: Size;
      env: [string, string | null][];
      /** Absent: as the item's last launch of `agent`; `""`: the agent's default. */
      model?: string;
    };

export type TermColors = { fg: [number, number, number]; bg: [number, number, number]; ansi: [number, number, number][] };

/** Fire-and-forget messages. */
export type Notice =
  | { t: "input"; session: SessionId; data: number[] }
  | { t: "resize"; session: SessionId; size: Size }
  /** The colors this session's programs are told the terminal has (OSC 10/11/4). */
  | { t: "term_colors"; session: SessionId; colors: TermColors | null }
  /** To `valk web` itself, not the daemon: whether this app is on screen, which
   * holds back notifications. */
  | { t: "visible"; visible: boolean }
  /** To `valk web`: follow this session's agent transcript (`null`: stop). */
  | { t: "chat"; session: SessionId | null };

export type Reply =
  | { t: "done" }
  /** `web`: the build of the app `valk web` serves (see lib/build.ts); `app_protocol`:
   * the protocol that build speaks (see lib/protocol.ts). Both added by `valk web`. */
  | {
      t: "hello";
      protocol: number;
      generation: number;
      boot: number;
      web?: string;
      app_protocol?: number;
    }
  | { t: "session"; info: SessionInfo }
  | { t: "sessions"; sessions: SessionInfo[] }
  | { t: "text"; text: string }
  | { t: "decision"; decision: Decision }
  | { t: "decisions"; decisions: Decision[] }
  | { t: "doc"; doc: Doc }
  /** `to`: on a new comment, the running session that hears it at its next prompt. */
  | { t: "doc_comments"; comments: DocComment[]; to?: string }
  | { t: "docs"; docs: Doc[]; root: string; unread?: number; at?: number }
  /** `conflict`: refused, and `doc` is what's on disk now. */
  | { t: "doc_saved"; doc: Doc; conflict?: boolean }
  | { t: "doc_history"; versions: DocVersion[]; text?: string }
  | { t: "doc_deleted"; token: string }
  | { t: "deleted_docs"; docs: DeletedDoc[] }
  /** `note`: what else happened (a worktree taken away, or why it was kept). */
  | { t: "item"; item: Item; note?: string }
  | { t: "items"; items: Item[]; orders?: OrderProposal[]; launches?: LaunchOffer[] }
  | { t: "item_facts"; facts: ItemFacts | null }
  | { t: "order"; order: OrderProposal }
  | { t: "order_applied"; applied: OrderApplied }
  | { t: "step_taken"; said: string }
  | { t: "shipped"; shipped: ShippedItem[]; kept: KeptItem[] }
  /** `from_top`: the first row's line, from the oldest in history; `history`: lines
   * above the screen, so `from_top == history` is the live screen. */
  | { t: "scrollback"; from_top: number; history: number; rows: Row[] }
  | { t: "digest"; digest: Digest }
  | { t: "history"; sessions: PastSession[] }
  | { t: "search"; results: SearchResults };

export type ServerMsg =
  | { t: "ok"; req: number; reply: Reply }
  | { t: "err"; req: number; message: string }
  | { t: "screen"; session: SessionId; update: ScreenUpdate }
  | { t: "exited"; session: SessionId; code: number | null }
  | { t: "queue"; items: QueueItem[] }
  /** Every project's decisions waiting on review, oldest first. */
  | { t: "proposals"; items: Decision[] }
  /** Every backlog item, and the build orders and launch offers waiting on the user. */
  | { t: "items"; items: Item[]; orders?: OrderProposal[]; launches?: LaunchOffer[] }
  /** The plan's quota, if anything has said it yet (protocol 30). */
  | { t: "quota"; quota: Quota | null }
  | { t: "clipboard"; session: SessionId; text: string }
  | { t: "graphics"; session: SessionId }
  | ChatUpdate;

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
