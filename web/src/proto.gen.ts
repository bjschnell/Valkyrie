// Generated from crates/valkyrie-proto by its `proto_ts` test: don't edit it.
// After changing a wire type, run `UPDATE_PROTO_TS=1 cargo test -p valkyrie-proto proto_ts`.
// web/src/proto.ts re-exports these and adds what only the web app sends or reads.

export type AgentState = "working" | "needs_input" | "blocked" | "review_ready" | "idle" | "interrupted" | "stale" | "exited";

export type AgentStatus = { 
/**
 * Adapter name: `claude`, `codex` or `generic`.
 */
agent: string, state: AgentState, ask: AskKind | null, 
/**
 * One line: what it needs, or what it finished.
 */
summary: string | null, 
/**
 * When the session entered `state` (ms since the epoch).
 */
since_ms: number, 
/**
 * Bumped on every transition; `MarkSeen` names the transition it saw.
 */
seq: number, 
/**
 * The human has looked at this transition (attached, or marked it seen).
 */
seen: boolean, 
/**
 * At least one hook event arrived, so state isn't heuristics only.
 */
hooked: boolean, };

/**
 * What a `NeedsInput` session is waiting for.
 */
export type AskKind = "permission" | "question" | "input" | "screen" | "bell";

/**
 * Which sessions' corrections are read for decisions (DESIGN §6.3).
 */
export type AutoMode = "off" | "claude" | "all";

export type Axis = "columns" | "rows";

/**
 * A client that stays connected, as its `Hello` names it.
 */
export type ClientBuild = { 
/**
 * `tui` or `web`.
 */
kind: string, pid: number, 
/**
 * The protocol it speaks.
 */
protocol: number, 
/**
 * Its binary, from `binary_id`; `None` where that's unknown (Windows).
 */
binary: [number, number] | null, };

export type ClientMsg = { "t": "hello", req: number, 
/**
 * Sent by the clients that stay connected (the TUI, `valk web`), so the
 * daemon can tell `valk upgrade` which ones run an older build.
 */
client?: ClientBuild | null, }
  | { "t": "clients", req: number, }
  | { "t": "spawn", req: number, spec: SpawnSpec, }
  | { "t": "list", req: number, }
  | { "t": "kill", req: number, session: number, }
  | { "t": "attach", req: number, session: number, size: Size | null, }
  | { "t": "attach_panes", req: number, panes: Array<[number, Size]>, }
  | { "t": "detach", req: number, }
  | { "t": "split", req: number, session: number, side: Side, spec: SpawnSpec, }
  | { "t": "ratio", req: number, a: number, b: number, ratio: number, }
  | { "t": "dump", req: number, session: number, }
  | { "t": "scrollback", req: number, session: number, anchor: ScrollAnchor, }
  | { "t": "input", session: number, data: Array<number>, }
  | { "t": "resize", session: number, size: Size, }
  | { "t": "cell_pixels", session: number | null, width: number, height: number, }
  | { "t": "term_colors", session: number | null, colors: TermColors | null, }
  | { "t": "hook", session: number, agent: string, 
/**
 * When the hook process sent it (µs since the epoch); orders racing hooks.
 */
sent_us: number, payload: JsonValue, }
  | { "t": "quota", quota: Quota, }
  | { "t": "watch_queue", req: number, }
  | { "t": "watch_notices", req: number, shows: boolean, }
  | { "t": "notify", req: number, text: string, sound: NoticeSound | null, }
  | { "t": "mark_seen", req: number, session: number, seq: number, }
  | { "t": "move", req: number, session: number, to: number, }
  | { "t": "report", req: number, session: number, text: string | null, ttl_secs: number, }
  | { "t": "rename", req: number, session: number, name: string | null, }
  | { "t": "upgrade", req: number, exe: string, }
  | { "t": "decide", req: number, decision: NewDecision, }
  | { "t": "review", req: number, project: string, id: number, action: ReviewAction, 
/**
 * The decision's `updated` as the reviewer saw it: refused if it changed
 * since, so what's accepted is what was read.
 */
seen: number | null, }
  | { "t": "decisions", req: number, cwd: string | null, }
  | { "t": "auto", req: number, mode: AutoMode | null, }
  | { "t": "siblings", req: number, session: number, }
  | { "t": "vouch", req: number, what: string, }
  | { "t": "add_item", req: number, item: NewItem, 
/**
 * Also take it for the caller's session, with its branch and pull request
 * (protocol 18): an agent tracking work it does that nobody planned. From
 * inside a valk session only.
 */
take?: boolean, branch?: string | null, pr?: string | null, }
  | { "t": "items", req: number, }
  | { "t": "change_item", req: number, id: number, change: ItemChange, }
  | { "t": "delete_item", req: number, id: number, }
  | { "t": "note_item", req: number, id: number, text: string, }
  | { "t": "item_facts", req: number, id: number, }
  | { "t": "answer_launch", req: number, project: string, id: number, launch: boolean, }
  | { "t": "supervisor_log", req: number, }
  | { "t": "supervisor_watch", req: number, }
  | { "t": "supervise", req: number, on: boolean | null, }
  | { "t": "web_server", req: number, on: boolean | null, }
  | { "t": "supervisor_session", req: number, cwd: string, size: Size, env: Array<[string, string | null]>, }
  | { "t": "take_step", req: number, session: number, seq: number, action: StepAction, }
  | { "t": "take_item", req: number, id: number, branch?: string | null, pr?: string | null, steal?: boolean, }
  | { "t": "adopt_session", req: number, session: number, title?: string | null, kind: ItemKind, }
  | { "t": "my_item", req: number, }
  | { "t": "ship_merged", req: number, dry_run: boolean, only?: Array<number> | null, }
  | { "t": "launch_item", req: number, id: number, 
/**
 * `claude` or `codex`.
 */
agent: string, worktree: boolean, size: Size, env: Array<[string, string | null]>, 
/**
 * The model to run (protocol 20): `None` keeps the one the item's last
 * launch of `agent` ran, `""` is the agent's own default.
 */
model?: string | null, }
  | { "t": "free_disk", req: number, }
  | { "t": "prune_worktrees", req: number, cwd: string, 
/**
 * Count `origin`'s default branch as merged into; false when the client
 * couldn't fetch it, so only the main checkout's `HEAD` counts.
 */
remote: boolean, remove: boolean, }
  | { "t": "write_doc", req: number, doc: NewDoc, }
  | { "t": "read_doc", req: number, which: string, }
  | { "t": "edit_doc", req: number, id: number, base: string, body: string, }
  | { "t": "docs", req: number, count_only?: boolean, session?: number | null, }
  | { "t": "open_doc", req: number, key: string, }
  | { "t": "docs_seen", req: number, at: number, }
  | { "t": "doc_comments", req: number, key: string, }
  | { "t": "comment_doc", req: number, key: string, quote?: string | null, text: string, }
  | { "t": "resolve_comment", req: number, key: string, comment: number, }
  | { "t": "save_doc", req: number, key: string, base: string, body: string, }
  | { "t": "doc_history", req: number, key: string, version?: string | null, }
  | { "t": "restore_doc", req: number, key: string, base: string, version: string, }
  | { "t": "delete_doc", req: number, key: string, base: string, }
  | { "t": "undelete_doc", req: number, token: string, }
  | { "t": "deleted_docs", req: number, }
  | { "t": "docs_root", req: number, root?: string | null, }
  | { "t": "digest", req: number, since_ms?: number | null, by: DigestGroup, }
  | { "t": "search", req: number, query: string, since_ms?: number | null, 
/**
 * Only sessions in this repository (its folder name).
 */
repo?: string | null, limit?: number | null, }
  | { "t": "note", req: number, note: NewNote, }
  | { "t": "notes", req: number, cwd: string | null, }
  | { "t": "note_rm", req: number, cwd: string, id: number, }
  | { "t": "wiki", req: number, cwd: string, }
  | { "t": "wiki_page", req: number, cwd: string, page: string, }
  | { "t": "wiki_explore", req: number, cwd: string, stale: boolean, agent: string, size: Size, 
/**
 * As `SpawnSpec::env`.
 */
env: Array<[string, string | null]>, }
  | { "t": "tutor", req: number, agent: string, size: Size, 
/**
 * As `SpawnSpec::env`.
 */
env: Array<[string, string | null]>, }
  | { "t": "served_to", req: number, project: string, id: number, }
  | { "t": "repos", req: number, }
  | { "t": "repo_edit", req: number, edit: RepoEdit, }
  | { "t": "history", req: number, }
  | { "t": "resume", req: number, key: string, size: Size, 
/**
 * As `SpawnSpec::env`.
 */
env: Array<[string, string | null]>, };

export type Color = "default" | { "indexed": number }
  | { "rgb": [number, number, number] };

/**
 * Where a comment on a doc stands (ADR-0021 §6).
 */
export type CommentState = "open" | "sent" | "answered" | "resolved";

export type ConnectedClient = { client: ClientBuild, 
/**
 * It speaks the daemon's protocol and runs the daemon's binary (as far as
 * either side knows its binary).
 */
current: boolean, };

export type Cursor = { x: number, y: number, visible: boolean, shape: CursorShape, };

export type CursorShape = "block" | "underline" | "beam";

export type Decision = { 
/**
 * Numbered from 1 within its project.
 */
id: number, 
/**
 * The project's root (ADR-0007 §2).
 */
project: string, title: string, body: string, kind: DecisionKind, status: DecisionStatus, 
/**
 * Seconds since the epoch.
 */
created: number, updated: number, supersedes?: number | null, superseded_by?: number | null, 
/**
 * The user wants its body in every session's block (ADR-0014 §1). Only a
 * human sets it, as only a human accepts.
 */
pinned?: boolean, 
/**
 * Path globs relative to the project root (`crates/valkyrie-tui/**`) it
 * applies to (ADR-0014 §2): its body reaches an agent when it first touches
 * a file in them. Empty: it holds everywhere, and has no file of its own.
 */
scope?: Array<string>, 
/**
 * A scope proposed for it once, waiting on the user (ADR-0014 §2).
 */
proposed_scope?: Array<string> | null, 
/**
 * Its scope is settled: it was recorded with one, or a scope was proposed
 * for it once, so `valk decisions scope --propose` passes it over.
 */
scope_asked?: boolean, provenance: Provenance, fresh: Freshness, };

/**
 * What sort of thing a decision records. Constraints rank first when injected; a
 * skill is a procedure, listed by title and read in full when needed (ADR-0011);
 * a policy picks a template Valkyrie's hooks enforce (ADR-0013), its body
 * settings, not prose.
 */
export type DecisionKind = "decision" | "constraint" | "pattern" | "gotcha" | "fix" | "skill" | "policy";

export type DecisionStatus = "proposed" | "active" | "rejected" | "superseded" | "retired";

/**
 * A doc the user deleted, kept so it can be put back.
 */
export type DeletedDoc = { 
/**
 * What `UndeleteDoc` takes.
 */
token: string, 
/**
 * When it was deleted, in seconds.
 */
at: number, 
/**
 * Where it was, relative to the docs folder.
 */
path: string, title: string, };

export type Digest = { 
/**
 * The window (ms since the epoch).
 */
since_ms: number, until_ms: number, 
/**
 * `since_ms` is when you were last gone, not a time you gave.
 */
away: boolean, by: DigestGroup, 
/**
 * Most urgent first, then most recent.
 */
entries: Array<DigestEntry>, 
/**
 * The same, as markdown to paste: a standup, a message.
 */
markdown: string, };

/**
 * One agent session in the window.
 */
export type DigestEntry = { 
/**
 * Its event log's name (`<created ns>-<id>`): stable across daemon restarts.
 */
key: string, 
/**
 * The live session, while it runs in this daemon.
 */
session?: number | null, 
/**
 * The repository's folder name, or the directory's outside git.
 */
repo: string, cwd?: string | null, 
/**
 * `claude`, `codex`, ...
 */
agent: string, 
/**
 * What it was about, one line: its backlog item, topic, name or first prompt.
 */
what: string, item?: number | null, branch?: string | null, pr?: string | null, outcome: Outcome, 
/**
 * Why that outcome, one line ("PR #12 merged", "CI red on PR #9: lint").
 */
detail: string, 
/**
 * The agent's own last line, when it said one.
 */
summary?: string | null, 
/**
 * Prompts it was given in the window.
 */
prompts: number, 
/**
 * First and last activity in the window (ms since the epoch).
 */
first_ms: number, last_ms: number, 
/**
 * When it ended, if it did.
 */
ended_ms?: number | null, };

/**
 * How the digest is grouped.
 */
export type DigestGroup = "repo" | "day";

/**
 * A doc as listed or read.
 */
export type Doc = { 
/**
 * Its `valk-id`; 0 for a file of the user's that hasn't been read through
 * Valkyrie yet.
 */
id: number, kind: DocKind, title: string, 
/**
 * Relative to the docs folder.
 */
path: string, 
/**
 * What the web app opens it by (ADR-0021 §5): its `valk-id`, or for one of the
 * user's own docs with no id, a handle the daemon made from its path. Never a
 * path itself. Only in a list or a web read.
 */
key?: string, 
/**
 * An agent wrote or edited it since the user last opened the Docs list. Only
 * in a list.
 */
unread?: boolean, 
/**
 * The agent that wrote it, or `human`.
 */
by: string, created: string, session?: string | null, commit?: string | null, item?: number | null, repo?: string | null, 
/**
 * Who edited it since, one line each.
 */
edited?: Array<string>, 
/**
 * The whole file's hash, which an edit names as its base; with `text` only.
 */
hash?: string | null, 
/**
 * The whole file, front matter and all; only when read or written.
 */
text?: string | null, };

/**
 * A comment the user left on a doc, or on one block of it.
 */
export type DocComment = { 
/**
 * Its number within the doc, from 1.
 */
id: number, 
/**
 * When it was left, in seconds.
 */
at: number, 
/**
 * The block it's about, as it read then; none for the whole doc.
 */
quote?: string | null, text: string, state: CommentState, 
/**
 * The session it was told to, by name.
 */
sent_to?: string | null, 
/**
 * When it last changed state, in seconds.
 */
changed: number, };

/**
 * What a doc is for. A front matter field, not a folder, so agents never pick one.
 */
export type DocKind = "plan" | "research" | "report" | "note";

/**
 * One of a doc's past versions (ADR-0021 §4): the file as it was before an edit
 * replaced it.
 */
export type DocVersion = { 
/**
 * What `DocHistory` and `RestoreDoc` name it by.
 */
version: string, 
/**
 * When it was replaced, in seconds.
 */
at: number, 
/**
 * Who wrote it: its last `edited` line, or its author.
 */
by: string, 
/**
 * Its size in bytes.
 */
size: number, };

/**
 * A watcher's flag on an idle session (ADR-0019 §1): the reason the queue shows,
 * and the one step the supervisor proposes for it.
 */
export type Flag = { 
/**
 * "PR #98 conflicts with main", "CI red on PR #101: windows".
 */
reason: string, 
/**
 * The pull request it's about.
 */
pr: string, 
/**
 * The status transition the flag raised: `TakeStep` names it, so a tap on a
 * flag that has moved on does nothing.
 */
seq: number, 
/**
 * `None` while the supervisor works out which step to offer (`working`), and
 * once one was taken or the user left the flag.
 */
step?: Step | null, 
/**
 * The supervisor is still working out its step (the flake call can take a
 * while).
 */
working?: boolean, };

/**
 * Whether a decision may be out of date (DESIGN §6.4).
 */
export type Freshness = { 
/**
 * When a human last said it holds (seconds); 0: when it was made active.
 */
confirmed: number, 
/**
 * Days after `confirmed` that it's due for review again, for decisions about
 * things that change (versions, endpoints).
 */
review_every?: number | null, 
/**
 * Files it's about, relative to the project root: if they change a lot, it may
 * no longer hold.
 */
anchors?: Array<string>, 
/**
 * Why it should be looked at again, when it should; worked out by the daemon,
 * never stored.
 */
review?: string | null, };

export type Item = { 
/**
 * Numbered from 1 across every project, so `valk todo launch 12` needs no folder.
 * The key everything stores and sends; what people read and type is `number`.
 */
id: number, 
/**
 * The number within its repository, from 1: the `#N` a person sees. Items with
 * no repository share one count. 0 only on an item written before there were
 * numbers; the backlog fills those in when it loads them.
 */
number: number, title: string, body: string, kind: ItemKind, status: ItemStatus, priority: Priority, 
/**
 * Items to finish first. Advisory: one still open sorts this below the ready
 * ones and is named in its brief, but nothing refuses a take or launch.
 */
blocked_by?: Array<number>, 
/**
 * The item it's a piece of (backlog #235): one level deep, in the same
 * repository. A child is a whole item of its own (branch, pull request,
 * ship); the parent groups them, and ships itself once every child is shipped
 * or dropped and its own pull request, if it has one, has merged.
 */
parent?: number | null, 
/**
 * Its pull request merged while some of its children were still open: it
 * waits for them instead of shipping.
 */
pr_merged?: boolean, 
/**
 * The repository it's for; `None` until one is picked.
 */
project?: string | null, 
/**
 * Screenshots and files, copied into the backlog's own folder.
 */
attachments?: Array<string>, 
/**
 * `human`, or the agent that wrote it down.
 */
by: string, 
/**
 * Sessions started on it, oldest first. Ids are only meaningful to the daemon
 * that started them: a restarted one reuses them, so clients match on `boot`.
 */
sessions?: Array<number>, 
/**
 * The daemon `boot` that `sessions` belong to.
 */
boot: number, 
/**
 * Those of `sessions` that took it (`valk todo take`) rather than being
 * launched on it: open to any agent, so their notes stay out of the brief.
 */
taken?: Array<number>, 
/**
 * When each of `sessions` last took or was launched on it, in seconds: the
 * item a session is on is the one it joined last, which a note on an older
 * one doesn't change (`updated` does). Pairs, not a map: a map's number keys
 * are strings in JSON, which a tagged message can't read back as numbers.
 */
joined?: Array<[number, number]>, 
/**
 * The branch and checkout its first launch made, reused by later ones; or
 * the branch a take recorded (`branch_taken`), with no checkout.
 */
branch?: string | null, 
/**
 * `branch` was set by `valk todo take`, not a launch: the user may replace it.
 */
branch_taken?: boolean, checkout?: string | null, 
/**
 * The pull request, an `https://` URL, as `valk todo take --pr` recorded it.
 */
pr?: string | null, 
/**
 * The daemon last read `pr` as closed without merging: it waits for the user
 * to drop or relaunch the item. Cleared when the PR reads open again, or a
 * take records another.
 */
pr_closed?: boolean, 
/**
 * The daemon `boot` and session of the agent that wrote it down (`valk todo
 * add`, with or without `--take`). That session may take it while it's still
 * proposed: its own find, which the user said to go ahead with in that
 * session (ADR-0018), or its own work tracked (ADR-0012).
 */
filed_by?: [number, number] | null, 
/**
 * Shipped or dropped while still proposed: the user never accepted it, so
 * reopening it makes it proposed again rather than open.
 */
unaccepted?: boolean, 
/**
 * Handoffs left on it, oldest first: what an agent did and what's left, or
 * the user's own.
 */
notes?: Array<ItemNote>, 
/**
 * The agent and model its last launch ran: the next launch of that agent
 * keeps the model unless it's given another.
 */
launch?: ItemLaunch | null, 
/**
 * Seconds since the epoch.
 */
created: number, updated: number, };

export type ItemChange = { "do": "edit", title?: string | null, body?: string | null, kind?: ItemKind | null, cwd?: string | null, priority?: Priority | null, 
/**
 * The whole list, replacing the one it has; empty clears it.
 */
blocked_by?: Array<number> | null, 
/**
 * The item it's a piece of (protocol 46); `0` makes it stand alone again.
 */
parent?: number | null, }
  | { "do": "status", status: ItemStatus, }
  | { "do": "return" };

/**
 * What an item's checkout holds, read from git rather than taken from the agent.
 */
export type ItemFacts = { branch: string, 
/**
 * Commits on it that the repository's own checkout doesn't have.
 */
ahead: number, 
/**
 * The newest of them, `<short hash> <subject>`, at most ten.
 */
commits: Array<string>, 
/**
 * `git diff --shortstat` against where it started, e.g. `3 files changed, 40
 * insertions(+), 2 deletions(-)`; empty when nothing changed.
 */
diff: string, 
/**
 * Files changed or untracked but not committed.
 */
dirty: number, };

/**
 * What sort of work an item is; it picks the brief its agent starts with.
 */
export type ItemKind = "bug" | "feature" | "chore" | "spike" | "debt";

/**
 * What an item was last launched with.
 */
export type ItemLaunch = { 
/**
 * `claude` or `codex`.
 */
agent: string, 
/**
 * Empty for the agent's own default.
 */
model?: string, };

export type ItemNote = { 
/**
 * Seconds since the epoch.
 */
at: number, 
/**
 * `human`, or the agent that left it.
 */
by: string, text: string, 
/**
 * Left by an agent on an item it took rather than was launched for: shown,
 * but never put into a launch's brief (ADR-0007).
 */
taken?: boolean, 
/**
 * Left by an agent on an item it was neither launched for nor took, from
 * work elsewhere: shown, never in a brief, and kept out of the prompts that
 * judge the item. `taken` is set with it, so older builds keep it out of a
 * brief too.
 */
aside?: boolean, 
/**
 * For an aside, the item that agent was on, when it was on one.
 */
from?: number | null, 
/**
 * `from`'s number in its repository, as people know it; 0 on a note from
 * before numbers, which then shows the id.
 */
from_number?: number, };

export type ItemStatus = "open" | "shipped" | "dropped" | "proposed";

export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;

/**
 * An open item with a branch that `ShipMerged` left open, and why.
 */
export type KeptItem = { id: number, 
/**
 * Its number in its repository; 0 for an item that's gone.
 */
number: number, title: string, 
/**
 * `not merged`, `branch fix/x not found`, `no commits of its own`, …
 */
why: string, };

/**
 * One repository a picker offers: a main checkout's root, never a worktree.
 */
export type KnownRepo = { path: string, 
/**
 * `~/repos/valkyrie`, as it reads.
 */
label: string, 
/**
 * Added by hand, rather than found.
 */
added: boolean, };

/**
 * A ready item the supervisor offers to start (ADR-0019 §1, §2 `launch`): open,
 * nobody on it, its blockers shipped or dropped (or the model judged it doesn't
 * need the one left), with room under the repository's `max_running`. One per
 * repository at most. The user starts it or leaves it (`ClientMsg::AnswerLaunch`);
 * once they've started two this way, a policy can start them without asking.
 */
export type LaunchOffer = { 
/**
 * Changes whenever the repository's offer is replaced, so an answer meant for
 * an older one is refused.
 */
id: number, 
/**
 * The repository's root.
 */
project: string, item: number, 
/**
 * The item's number in its repository, as people know it.
 */
number: number, 
/**
 * The item's title, for a push or a banner.
 */
title: string, 
/**
 * The agent and model it starts on: the item's last launch, else claude on
 * its own default model.
 */
agent: string, model?: string, 
/**
 * One line: why this item, now.
 */
why: string, 
/**
 * Unfinished blockers the model judged it doesn't need first.
 */
waived?: Array<number>, 
/**
 * Seconds since the epoch.
 */
made: number, };

/**
 * A disk running low on space.
 */
export type LowDisk = { 
/**
 * A folder on it the daemon looked at: its state folder or a repository.
 */
path: string, 
/**
 * Bytes free for an ordinary user, and the disk's size.
 */
free: number, total: number, };

/**
 * Terminal modes a client must mirror on its own terminal so that the raw input
 * bytes it forwards are encoded the way the program expects.
 */
export type Modes = { app_cursor: boolean, app_keypad: boolean, bracketed_paste: boolean, focus_events: boolean, alt_screen: boolean, 
/**
 * Mouse reporting the program asked for (1000/1002/1003) and its encoding
 * (1006 SGR, 1005 UTF-8). With none, the client may use the mouse itself.
 */
mouse_click: boolean, mouse_drag: boolean, mouse_motion: boolean, mouse_sgr: boolean, mouse_utf8: boolean, 
/**
 * 1007: the wheel scrolls by sending arrow keys in the alternate screen.
 */
alt_scroll: boolean, };

/**
 * A decision as asked for by `valk decide`, the TUI or the web app.
 */
export type NewDecision = { 
/**
 * Any directory inside the project.
 */
cwd: string, title: string, body: string, kind: DecisionKind, 
/**
 * Ask for review even when a human records it.
 */
propose: boolean, supersedes?: number | null, 
/**
 * Review it again this many days after it's confirmed; `None`: chosen from
 * what it says (30 days for versions and URLs, else never).
 */
review_every?: number | null, 
/**
 * The session it was recorded from (`$VALK_SESSION`). A session running an
 * agent can only propose (ADR-0007 §4).
 */
session?: number | null, 
/**
 * Path globs it applies to (ADR-0014 §2); `None`: the paths its text names.
 */
scope?: Array<string> | null, };

/**
 * A doc to write. Who wrote it, and from which session and commit, the daemon
 * works out itself.
 */
export type NewDoc = { 
/**
 * Where the writer is: a doc from inside a repository goes in its folder.
 */
cwd: string, title: string, kind: DocKind, 
/**
 * Agent-level, at the top of the docs folder, wherever `cwd` is.
 */
global: boolean, body: string, };

export type NewItem = { 
/**
 * May be empty: then the body's first line is the title.
 */
title: string, body: string, kind: ItemKind, priority?: Priority, blocked_by?: Array<number>, 
/**
 * The item it's a piece of; it goes in that item's repository, whatever `cwd` says.
 */
parent?: number | null, 
/**
 * A folder inside the repository it's for.
 */
cwd?: string | null, 
/**
 * Files to keep with it (an upload's path, or any file the daemon can read).
 */
attachments?: Array<string>, };

/**
 * A note to write, as a client asks for it. Where it came from is the daemon's to
 * work out, never the client's (ADR-0017 rule 1).
 */
export type NewNote = { cwd: string, text: string, 
/**
 * The note this one corrects: it becomes `replaced` in the same write.
 */
replaces: number | null, 
/**
 * Write it even though it matches a deleted note. Only a human may.
 */
force?: boolean, };

/**
 * One note, as stored in `<state dir>/context/<project>/notes/NNNN.md`.
 */
export type Note = { id: number, 
/**
 * The repository it belongs to. Not stored in the file: the directory says.
 */
project: string, text: string, status: NoteStatus, created: number, 
/**
 * `agent`, `user` or `valkyrie`.
 */
by: string, 
/**
 * The Valkyrie session it was written in, or `none`.
 */
session: string, conversation: string | null, 
/**
 * The checkout's `HEAD` when it was written, read by the daemon itself.
 */
commit: string | null, cwd: string | null, 
/**
 * The files its text names, found when it was written.
 */
anchors: Array<string>, 
/**
 * The failure signature it came from, for a note the retro wrote.
 */
sig: string | null, replaces: number | null, replaced_by: number | null, 
/**
 * Why it may be out of date (ADR-0017 §6). Worked out when it's read, never
 * stored.
 */
stale?: string | null, };

/**
 * Where a note stands. Only a live one is ever served.
 */
export type NoteStatus = "live" | "replaced" | "promoted";

/**
 * A project's notes, every status, with its tombstones.
 */
export type NotesListing = { notes: Array<Note>, deleted: Array<Tombstone>, };

/**
 * Which ping sound a `Notify` makes, as the queue's own pings do: `request` for
 * something that needs you, `done` for something that finished.
 */
export type NoticeSound = "request" | "done";

/**
 * Where a session's work ended up. Declared most urgent first: lists sort by it.
 */
export type Outcome = "stuck" | "abandoned" | "waiting" | "working" | "shipped" | "done";

export type Pane = { "t": "leaf", session: number, }
  | { "t": "split", axis: Axis, 
/**
 * `a`'s share of the room left after the divider, in `RATIO_SCALE`ths.
 */
ratio: number, a: Pane, b: Pane, };

/**
 * An agent session that has ended, as the history lists it (backlog #150).
 */
export type PastSession = { 
/**
 * When it started (ms since the epoch).
 */
started_ms: number, 
/**
 * Why it can't be resumed; `None`: it can.
 */
cannot?: string | null, 
/**
 * Its directory is gone: resuming adds its worktree back on this branch.
 */
regrow?: string | null, 
/**
 * Its event log's name (`<created ns>-<id>`): stable across daemon restarts.
 */
key: string, 
/**
 * The live session, while it runs in this daemon.
 */
session?: number | null, 
/**
 * The repository's folder name, or the directory's outside git.
 */
repo: string, cwd?: string | null, 
/**
 * `claude`, `codex`, ...
 */
agent: string, 
/**
 * What it was about, one line: its backlog item, topic, name or first prompt.
 */
what: string, item?: number | null, branch?: string | null, pr?: string | null, outcome: Outcome, 
/**
 * Why that outcome, one line ("PR #12 merged", "CI red on PR #9: lint").
 */
detail: string, 
/**
 * The agent's own last line, when it said one.
 */
summary?: string | null, 
/**
 * Prompts it was given in the window.
 */
prompts: number, 
/**
 * First and last activity in the window (ms since the epoch).
 */
first_ms: number, last_ms: number, 
/**
 * When it ended, if it did.
 */
ended_ms?: number | null, };

/**
 * An open pull request, as the poll read it.
 */
export type PrState = "green" | "pending" | "red" | "conflicting" | "unknown";

/**
 * How soon an item should be done: `P0` first. Within a lane, items are worked
 * in this order (`order`).
 */
export type Priority = "p0" | "p1" | "p2" | "p3";

/**
 * Where a decision came from.
 */
export type Provenance = { 
/**
 * `human`, or the agent that proposed it (`claude`, `codex`).
 */
by: string, 
/**
 * The session's name when it was recorded.
 */
session?: string | null, 
/**
 * The agent's own conversation id, to find the transcript.
 */
conversation?: string | null, 
/**
 * `HEAD` of the checkout it was recorded in.
 */
commit?: string | null, 
/**
 * The directory it was recorded in (a worktree, or below the root).
 */
cwd?: string | null, };

export type QueueItem = { session: number, name: string, 
/**
 * The name is the agent's topic, not one the user gave (`SessionInfo::auto_name`).
 */
auto_name?: boolean, cwd: string, status: AgentStatus, 
/**
 * What the supervisor flagged on this session's pull request after its last
 * stop, and the step it offers (ADR-0019 §1, §2). Only while the session still
 * shows that flag.
 */
flag?: Flag | null, 
/**
 * The session's published status (`SessionInfo::report`).
 */
report?: string | null, };

/**
 * The latest reading, from whichever Claude session last drew its status line.
 */
export type Quota = { 
/**
 * When it was read (seconds since the epoch): readings only come while a
 * Claude session runs, so a client shows how old it is.
 */
at: number, 
/**
 * The plan's windows, shortest first.
 */
limits: Array<QuotaLimit>, };

/**
 * One rate-limit window, e.g. the five-hour one or the week.
 */
export type QuotaLimit = { 
/**
 * Claude Code's own name for it: `five_hour`, `seven_day`, `seven_day_opus`, …
 */
name: string, 
/**
 * Share of the window used, 0 to 100.
 */
used: number, 
/**
 * When the window resets (seconds since the epoch), if said.
 */
resets_at?: number | null, };

export type Reply = { "t": "done" }
  | { "t": "hello", protocol: number, 
/**
 * How many upgrade handoffs this daemon's sessions have been through.
 */
generation: number, 
/**
 * Identifies this daemon across handoffs: a restarted daemon (whose session
 * ids start over) has a different one.
 */
boot: number, 
/**
 * Started with `valk daemon --dev`: a throwaway daemon for development,
 * whose web server lets any browser in with the fixed `dev-token`.
 */
dev: boolean, 
/**
 * The binary the daemon runs as: a client following an upgrade restarts as
 * this one, so it ends up on the daemon's build whichever valk it started
 * from. `None` from a daemon before it.
 */
exe?: string | null, }
  | { "t": "clients", clients: Array<ConnectedClient>, }
  | { "t": "session", info: SessionInfo, }
  | { "t": "sessions", sessions: Array<SessionInfo>, 
/**
 * The tabs with more than one pane; every other session is a tab of its own.
 */
layouts: Array<Pane>, }
  | { "t": "text", text: string, }
  | { "t": "notified", watchers: number, }
  | { "t": "decision", decision: Decision, }
  | { "t": "decisions", decisions: Array<Decision>, }
  | { "t": "doc", doc: Doc, }
  | { "t": "doc_saved", doc: Doc, conflict?: boolean, }
  | { "t": "doc_history", versions: Array<DocVersion>, 
/**
 * The asked-for version's whole text.
 */
text?: string | null, }
  | { "t": "doc_deleted", token: string, }
  | { "t": "deleted_docs", docs: Array<DeletedDoc>, }
  | { "t": "doc_comments", comments: Array<DocComment>, 
/**
 * On a new comment: the running session it will be told to at its next
 * prompt; none when the session that wrote the doc is gone.
 */
to?: string | null, }
  | { "t": "docs", docs: Array<Doc>, 
/**
 * The docs folder.
 */
root: string, 
/**
 * How many an agent wrote or edited since the list was last opened.
 */
unread: number, 
/**
 * When it was listed, in seconds, for `DocsSeen`.
 */
at: number, }
  | { "t": "item", item: Item, 
/**
 * What else happened, for the person who asked (a worktree taken away, or
 * why it was kept).
 */
note?: string | null, }
  | { "t": "items", items: Array<Item>, 
/**
 * Ready items the supervisor offers to start (protocol 27).
 */
launches?: Array<LaunchOffer>, }
  | { "t": "supervisor_log", entries: Array<SupervisorEntry>, on: boolean, }
  | { "t": "supervisor_watch", prs: Array<WatchedPr>, 
/**
 * When the poll last finished, seconds since the epoch; `None` before
 * its first pass.
 */
polled_at?: number | null, 
/**
 * When it reads them next, seconds since the epoch.
 */
next_at?: number | null, on: boolean, }
  | { "t": "step_taken", said: string, }
  | { "t": "item_facts", facts: ItemFacts | null, }
  | { "t": "shipped", shipped: Array<ShippedItem>, kept: Array<KeptItem>, }
  | { "t": "worktrees", 
/**
 * What merged was judged against, e.g. `origin/main` and `main at <repo>`;
 * empty when nothing is.
 */
against: Array<string>, worktrees: Array<Worktree>, }
  | { "t": "digest", digest: Digest, }
  | { "t": "history", sessions: Array<PastSession>, }
  | { "t": "search", results: SearchResults, }
  | { "t": "note", note: Note, }
  | { "t": "notes", listing: NotesListing, }
  | { "t": "tombstone", tombstone: Tombstone, }
  | { "t": "wiki", listing: WikiListing, }
  | { "t": "repos", list: RepoList, }
  | { "t": "served_to", sessions: Array<ServedTo>, }
  | { "t": "wiki_page", page: WikiPage, }
  | { "t": "scrollback", 
/**
 * The first row's line, counting from the oldest line in history.
 */
from_top: number, 
/**
 * Lines of history above the screen; `from_top == history` is the live screen.
 */
history: number, 
/**
 * One screenful, `y` from 0.
 */
rows: Array<Row>, };

/**
 * A change to the list. Paths may start with `~`.
 */
export type RepoEdit = { "do": "add", path: string, }
  | { "do": "hide", path: string, }
  | { "do": "unhide", path: string, }
  | { "do": "scan", path: string, }
  | { "do": "unscan", path: string, };

/**
 * What `Repos` replies.
 */
export type RepoList = { 
/**
 * Most recently worked in first.
 */
repos: Array<KnownRepo>, 
/**
 * The folders scanned.
 */
folders: Array<ScanFolder>, 
/**
 * Repositories the user hid, to offer back.
 */
hidden: Array<ScanFolder>, };

/**
 * What a human does to a decision.
 */
export type ReviewAction = { "a": "accept" }
  | { "a": "reject" }
  | { "a": "retire" }
  | { "a": "restore" }
  | { "a": "reopen" }
  | { "a": "confirm" }
  | { "a": "pin" }
  | { "a": "unpin" }
  | { "a": "edit", title: string, body: string, kind: DecisionKind, 
/**
 * The paths it applies to; `None` leaves them as they are.
 */
scope?: Array<string> | null, }
  | { "a": "revise", title: string, body: string, kind: DecisionKind, scope?: Array<string> | null, }
  | { "a": "scope", scope: Array<string>, }
  | { "a": "propose_scope", scope: Array<string>, }
  | { "a": "decline_scope" };

export type Row = { y: number, spans: Array<Span>, 
/**
 * The line continues on the next row (soft wrap), so copied text joins them.
 */
wrapped?: boolean, };

/**
 * A folder whose repositories (one level down) are offered.
 */
export type ScanFolder = { path: string, label: string, };

export type ScreenUpdate = { 
/**
 * True when `rows` holds every row and the client should discard its old state.
 */
full: boolean, size: Size, rows: Array<Row>, cursor: Cursor, modes: Modes, title: string | null, };

/**
 * Where a `Scrollback` page starts.
 */
export type ScrollAnchor = { "up": number }
  | { "from_top": number };

/**
 * Where in a session a hit was found.
 */
export type SearchField = "you" | "agent" | "file" | "about";

/**
 * One session that matched, at its best line.
 */
export type SearchHit = { 
/**
 * Its event log's name (`<created ns>-<id>`): stable across daemon restarts.
 */
key: string, 
/**
 * The live session, while it runs in this daemon: open it.
 */
session?: number | null, 
/**
 * The repository's folder name, or the directory's outside git.
 */
repo: string, cwd?: string | null, 
/**
 * `cwd` is gone (a removed worktree): a resume there can't start, and the
 * agent may not find its conversation from anywhere else.
 */
gone?: boolean, 
/**
 * `claude`, `codex`, ...
 */
agent: string, 
/**
 * What it was about, one line: its backlog item, topic, name or first prompt.
 */
what: string, item?: number | null, branch?: string | null, pr?: string | null, 
/**
 * Where the best line was found, and the line, clipped around the words.
 */
field: SearchField, snippet: string, 
/**
 * When that line was said (ms since the epoch).
 */
at_ms: number, 
/**
 * Lines in the session holding every word.
 */
lines: number, 
/**
 * Its last activity, and when it ended, if it did.
 */
last_ms: number, ended_ms?: number | null, 
/**
 * A closed session: the command that resumes its conversation, to run in
 * `cwd` (`claude --resume <id>`). `None` while it runs, or when its agent
 * can't resume.
 */
resume?: Array<string> | null, };

export type SearchResults = { query: string, 
/**
 * The window searched (ms since the epoch).
 */
since_ms: number, until_ms: number, 
/**
 * Newest first, at most the limit asked for.
 */
hits: Array<SearchHit>, 
/**
 * Sessions that matched, before the limit.
 */
total: number, };

/**
 * One session a decision reached (ADR-0014 §5), from the project's serving log,
 * for the decision's detail (backlog #256, protocol 50). Every serve to one
 * session is one of these.
 */
export type ServedTo = { 
/**
 * What the session was about, as History names it; the log's own session
 * string when its run is too old or not a known conversation.
 */
what: string, 
/**
 * The agent that ran it, when its run is known.
 */
agent?: string | null, 
/**
 * The session still running it, to open.
 */
session?: number | null, 
/**
 * How it reached it, each layer once, first served first: `block`, `title`,
 * `prompt`, `file`, `search`, `show`.
 */
layers: Array<string>, 
/**
 * Seconds since the epoch: the first and the latest serve.
 */
first: number, last: number, };

export type ServerMsg = { "t": "ok", req: number, reply: Reply, }
  | { "t": "err", req: number, message: string, }
  | { "t": "screen", session: number, update: ScreenUpdate, }
  | { "t": "exited", session: number, code: number | null, }
  | { "t": "notice", text: string, sound: NoticeSound | null, }
  | { "t": "queue", items: Array<QueueItem>, }
  | { "t": "proposals", items: Array<Decision>, }
  | { "t": "items", items: Array<Item>, 
/**
 * Ready items the supervisor offers to start, one per repository at most
 * (protocol 27).
 */
launches?: Array<LaunchOffer>, }
  | { "t": "quota", quota: Quota | null, }
  | { "t": "disk", low: Array<LowDisk>, }
  | { "t": "clipboard", session: number, text: string, }
  | { "t": "graphics", session: number, x: number, y: number, data: string, };

export type SessionInfo = { id: number, name: string, 
/**
 * `name` is what the agent says it works on, not one the user gave it: clients
 * show it set apart.
 */
auto_name?: boolean, command: Array<string>, cwd: string, pid: number | null, created_unix: number, title: string | null, clients: number, 
/**
 * `None` while running; `Some(code)` once the child exited (`code` may be unknown).
 */
exited: number | null | null, status: AgentStatus, 
/**
 * When a client last typed into it (ms since the epoch; 0: never). The web
 * app's server reads it to tell someone at a keyboard from someone away.
 */
last_input_ms: number, 
/**
 * The agent's own transcript (Claude's or Codex's JSONL log), when known: the
 * web app's Chat view reads it (DESIGN §8.7).
 */
chat?: string | null, 
/**
 * One line when another live agent in the same repository has changed the
 * same files, e.g. "overlaps session api on src/foo.rs" (ADR-0019 §1, protocol
 * 31). Shown beside the state, which it never changes.
 */
overlap?: string | null, 
/**
 * A short status the agent or a script published on it with `valk status`,
 * e.g. "tests 41/50" (backlog #117, protocol 39). Gone once its time is up.
 */
report?: string | null, 
/**
 * The supervisor session (ADR-0019 §10, protocol 45): there's at most one
 * running, and clients mark its tab.
 */
supervisor?: boolean, };

/**
 * An item `ShipMerged` shipped.
 */
export type ShippedItem = { id: number, 
/**
 * Its number in its repository.
 */
number: number, title: string, 
/**
 * The branch judged, and whether a take set it rather than a launch.
 */
branch: string, branch_taken?: boolean, 
/**
 * An agent filed it and the user hasn't accepted it: shipping it is the
 * acceptance (`Item::agent_filed`).
 */
filed?: boolean, 
/**
 * What its branch was found merged into, e.g. `origin/main`.
 */
against: string, 
/**
 * What else happened (its worktree taken away, or why it was kept).
 */
note?: string | null, };

/**
 * Where a new pane goes, beside the one it splits.
 */
export type Side = "up" | "down" | "left" | "right";

export type Size = { cols: number, rows: number, };

export type Span = { x: number, text: string, style?: Style, };

export type SpawnSpec = { command: Array<string>, cwd: string | null, name: string | null, size: Size, 
/**
 * The spawning client's login environment (`login_env`); `None` unsets a
 * variable the daemon inherited from whichever login started it.
 */
env: Array<[string, string | null]>, 
/**
 * Start in a new git worktree of `cwd`'s repository on this branch (DESIGN
 * §4.3); empty names the branch after the session. The daemon makes it.
 */
worktree?: string | null, 
/**
 * A session only its spawner knows: left out of lists, the queue and the
 * restore file, and killed when the connection that spawned it closes. The
 * TUI's mini terminal is one.
 */
hidden?: boolean, };

/**
 * The step offered for a flag (ADR-0019 §2): one action from a fixed list, why
 * it was picked, and the other one that would fit, if any. `leave` is always
 * there besides.
 */
export type Step = { action: StepAction, 
/**
 * One line: "every failed check matches #17", "conflicting".
 */
why: string, alt?: StepAction | null, };

/**
 * What the supervisor can do for a flagged session. Each is a fixed template;
 * no free text reaches an agent.
 */
export type StepAction = "merge-base" | "ci-fix" | "rerun" | "leave" | "launch";

export type Style = { fg?: Color, bg?: Color, flags?: number, };

/**
 * One thing the supervisor did, or offered and got an answer to.
 */
export type SupervisorEntry = { 
/**
 * Seconds since the epoch.
 */
at: number, 
/**
 * The repository it was in.
 */
project: string, 
/**
 * One line: "you took: fix CI on PR #101", "policy #12: started #45".
 */
said: string, 
/**
 * The policy that took it without asking; `None` when you answered.
 */
policy?: number | null, 
/**
 * The pull request it was about, as a URL.
 */
pr?: string | null, };

/**
 * What a program is told its terminal looks like: default text and background,
 * and the 16 ANSI colors (black to bright white), each as RGB.
 */
export type TermColors = { fg: [number, number, number], bg: [number, number, number], ansi: Array<[number, number, number]>, };

/**
 * What `valk notes rm` leaves behind in `notes/deleted.jsonl`, so nothing writes the
 * same note again (ADR-0017 rule 4). The text itself is gone: only hashes of its
 * words are kept, enough to recognise it.
 */
export type Tombstone = { id: number, sig: string | null, 
/**
 * Hashes of the words that carry its meaning, sorted.
 */
words: Array<string>, 
/**
 * `agent` or `user`.
 */
by: string, session: string, at: number, };

/**
 * What the PR poll last read of a pull request it watches (backlog #181,
 * protocol 44).
 */
export type WatchedPr = { url: string, 
/**
 * The repository it's in.
 */
project: string, 
/**
 * The backlog item it was read from; `None`: found from a session's branch.
 */
item?: number | null, branch?: string | null, state: PrState, 
/**
 * One line: "CI red on PR #101: lint", "checks still running".
 */
detail: string, 
/**
 * When it was read, seconds since the epoch.
 */
read_at: number, 
/**
 * Where it comes in the order to merge its repository's open PRs, from 1;
 * `None` when it's the only one there (backlog #215; optional, so no
 * protocol bump).
 */
order?: number | null, 
/**
 * One line on how it meets the repository's other open PRs: "merge after
 * #236: both change src/foo.rs"; `None` when it collides with none.
 */
merge_note?: string | null, };

/**
 * A repository's pages, as `Wiki` replies.
 */
export type WikiListing = { 
/**
 * The repository the pages are for.
 */
root: string, pages: Array<WikiPage>, };

/**
 * One page. In a listing `body` is empty; `WikiPage` fills it.
 */
export type WikiPage = { slug: string, title: string, summary: string, 
/**
 * The paths it covers, relative to the repository root.
 */
scope: Array<string>, 
/**
 * The commit it was written at, short.
 */
commit: string | null, 
/**
 * When it was written, Unix seconds.
 */
updated: number, 
/**
 * Why it may be out of date, if it may.
 */
stale: string | null, body: string, };

/**
 * A linked worktree as `PruneWorktrees` judged it.
 */
export type Worktree = { path: string, 
/**
 * `None`: a detached `HEAD`.
 */
branch: string | null, 
/**
 * Merged, clean, and nothing runs in it or is on it: it may go.
 */
prunable: boolean, removed: boolean, 
/**
 * Why it stays, or what taking it away did.
 */
why: string, 
/**
 * Ignored files and folders in it (`target/`, `.env`), which taking it away
 * deletes too.
 */
ignored: Array<string>, };
