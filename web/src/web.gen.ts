// Generated from crates/valkyrie-web by its `web_ts` test: don't edit it.
// After changing one of these types, run `UPDATE_WEB_TS=1 cargo test -p valkyrie-web web_ts`.
// web/src/proto.ts re-exports these beside the daemon's (proto.gen.ts).

/**
 * One entry in the Chat view.
 */
export type ChatItem = { "k": "user", id: string, text: string, at?: string, }
  | { "k": "say", id: string, text: string, at?: string, }
  | { "k": "tool", id: string, tool: string, summary: string, status: ToolStatus, detail: ToolDetail, }
  | { "k": "mark", id: string, text: string, at?: string, };

/**
 * A reset replaces the chat; otherwise items are added, or replace the item
 * with their id (a tool call that finished). Sent as `WebMsg::Chat`.
 */
export type ChatUpdate = { session: number, items: Array<ChatItem>, reset?: boolean, 
/**
 * Older items exist that weren't loaded.
 */
more?: boolean, 
/**
 * No transcript known for this session.
 */
missing?: boolean, 
/**
 * The session is gone.
 */
gone?: boolean, };

/**
 * What opening a card shows.
 */
export type ToolDetail = { file?: string, command?: string, diff?: string, content?: string, result?: string, };

export type ToolStatus = "running" | "ok" | "error" | "denied";

/**
 * What `valk web` adds to the daemon's hello: the build of the app it serves
 * (web/src/lib/build.ts), and the protocol that build speaks (lib/protocol.ts).
 */
export type WebHello = { web: string, app_protocol: number, };

/**
 * What `valk web` sends the app of its own, beside the daemon's frames.
 */
export type WebMsg = { "t": "chat" } & ChatUpdate;

/**
 * The app's own messages to `valk web`, which never reach the daemon.
 */
export type WebNotice = { "t": "visible", visible: boolean, }
  | { "t": "chat", session: number | null, };
