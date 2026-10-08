import { useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import type { ChatItem, SessionInfo } from "../proto";
import {
  blocks,
  cardBody,
  clock,
  diffKind,
  fileLine,
  groupSummary,
  parseInline,
  parseMarkdown,
  TOOL_STATUS_LABEL,
  type ToolItem,
} from "../lib/chat";
import { useApp } from "../store";
import { Chevron } from "./icons";

/**
 * The session's agent conversation (DESIGN §8.7): what you asked, what it said, and
 * each tool call as a card. Read from the agent's own transcript, so it is the
 * whole conversation, not the screenful the terminal still holds.
 *
 * Runs of tool calls fold into one line ("Ran 6 commands, edited 2 files"). On a
 * phone you want what it said and what it changed; the run is a tap away. The run
 * still going stays open, so you can watch it work.
 */
export function ChatView({
  info,
  scroller,
  onTerminal,
}: {
  info: SessionInfo | undefined;
  scroller: RefObject<HTMLDivElement | null>;
  onTerminal: () => void;
}) {
  const chat = useApp((s) => s.chat);
  const now = useApp((s) => s.now);
  const stick = useRef(true);
  const list = useMemo(() => blocks(chat?.items ?? []), [chat]);
  const working = info?.status.state === "working";
  const waiting = info?.status.state === "needs_input" || info?.status.state === "blocked";

  const onScroll = () => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  };
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [chat, working, waiting, scroller]);

  if (!chat) return <div ref={scroller} className="chat loading">Opening…</div>;
  if (chat.missing && chat.items.length === 0) {
    return (
      <div ref={scroller} className="chat empty">
        <p>No conversation to show yet.</p>
        <p className="muted">
          Chat reads Claude Code's and Codex's own transcripts. This session hasn't named one, so start a
          prompt, or use the terminal.
        </p>
        <button className="primary" onClick={onTerminal}>
          Show the terminal
        </button>
      </div>
    );
  }

  return (
    <div ref={scroller} className="chat" onScroll={onScroll}>
      <div className="chat-inner">
        {chat.more && <div className="chat-more">Earlier messages aren't loaded</div>}
        {list.map((b, i) =>
          b.k === "tools" ? (
            <ToolGroup key={b.id} tools={b.tools} live={working && i === list.length - 1} />
          ) : (
            <Entry key={b.item.id} item={b.item} now={now} />
          ),
        )}
        {working && (
          <div className="chat-working">
            <span className="spin" /> Working…
          </div>
        )}
        {info?.status.state === "review_ready" && (
          <div className="chat-done">
            Finished.{" "}
            <a className="link" href={`#/s/${info.id}/review`}>
              Review the changes
            </a>
          </div>
        )}
        {waiting && (
          <div className="chat-waiting">
            Waiting on you{info?.status.summary ? `: ${info.status.summary}` : ""}.{" "}
            <button className="link" onClick={onTerminal}>
              Open the terminal
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

function Entry({ item, now }: { item: ChatItem; now: number }) {
  switch (item.k) {
    case "user":
      return (
        <div className="msg msg-user">
          <div className="bubble">{item.text}</div>
          <div className="msg-time">{clock(item.at, now)}</div>
        </div>
      );
    case "say":
      return (
        <div className="msg msg-say">
          <Markdown text={item.text} />
        </div>
      );
    case "mark":
      return (
        <div className="chat-mark">
          <span>{item.text}</span>
          {item.at && <span className="muted"> · {clock(item.at, now)}</span>}
        </div>
      );
    case "tool":
      return <ToolCard card={item} />;
  }
}

function ToolGroup({ tools, live }: { tools: ToolItem[]; live: boolean }) {
  const [open, setOpen] = useState<boolean | null>(null);
  const shown = open ?? live;
  const running = tools.some((t) => t.status === "running");
  return (
    <div className={`tool-group ${shown ? "open" : ""}`}>
      <button className="tool-group-head" aria-expanded={shown} onClick={() => setOpen(!shown)}>
        <Chevron />
        <span className="tool-group-line">{groupSummary(tools)}</span>
        {running && <span className="spin" />}
      </button>
      {shown && (
        <div className="tool-group-body">
          {tools.map((t) => (
            <ToolCard key={t.id} card={t} />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One tool call, closed by default: which tool, what it touched, how it went. The
 * status is a word, not only a colour; `denied` matters most, since the agent
 * carried on without it.
 */
function ToolCard({ card }: { card: ToolItem }) {
  const [open, setOpen] = useState(false);
  const body = cardBody(card);
  const file = fileLine(card);
  return (
    <div className={`tool-card tool-${card.status}`}>
      <button className="tool-head" onClick={() => setOpen((v) => !v)} disabled={!body} aria-expanded={body ? open : undefined}>
        <span className="tool-name">{card.tool}</span>
        {file ? (
          <span className="tool-summary">
            <span className="tool-file">{file.name}</span>
            {file.lines !== null && <span className="diff-add"> +{file.lines}</span>}
            {(file.added > 0 || file.removed > 0) && (
              <>
                <span className="diff-add"> +{file.added}</span>
                <span className="diff-del"> −{file.removed}</span>
              </>
            )}
            {file.dir && <span className="tool-dir"> {file.dir}</span>}
          </span>
        ) : (
          <span className="tool-summary">{card.summary}</span>
        )}
        <span className="tool-status">
          {card.status === "running" ? <span className="spin" /> : TOOL_STATUS_LABEL[card.status]}
        </span>
      </button>
      {open && body && (
        <div className="tool-body">
          {body.map((part) => (
            <pre key={part.label} className={`tool-pre ${part.label}`} aria-label={part.label}>
              {part.label === "diff" ? <Diff text={part.text} /> : part.text}
            </pre>
          ))}
        </div>
      )}
    </div>
  );
}

function Diff({ text }: { text: string }) {
  return (
    <>
      {text.split("\n").map((line, i) => (
        <span key={i} className={`diff-${diffKind(line)}`}>
          {line}
          {"\n"}
        </span>
      ))}
    </>
  );
}

function Markdown({ text }: { text: string }) {
  const parsed = useMemo(() => parseMarkdown(text), [text]);
  return (
    <div className="md">
      {parsed.map((b, i) => {
        switch (b.t) {
          case "p":
            return <p key={i}>{inline(b.text)}</p>;
          case "h":
            return <div key={i} className={`md-h md-h${Math.min(b.level, 3)}`}>{inline(b.text)}</div>;
          case "code":
            return (
              <pre key={i} className="md-code">
                {b.text}
              </pre>
            );
          case "pre":
            return (
              <pre key={i} className="md-code md-table">
                {b.text}
              </pre>
            );
          case "list": {
            const List = b.ordered ? "ol" : "ul";
            return (
              <List key={i}>
                {b.items.map((item, j) => (
                  <li key={j}>{inline(item)}</li>
                ))}
              </List>
            );
          }
          case "quote":
            return <blockquote key={i}>{inline(b.text)}</blockquote>;
          case "hr":
            return <hr key={i} />;
        }
      })}
    </div>
  );
}

function inline(text: string): ReactNode[] {
  return parseInline(text).map((part, i) => {
    switch (part.t) {
      case "text":
        return part.v;
      case "code":
        return <code key={i}>{part.v}</code>;
      case "b":
        return <strong key={i}>{part.v}</strong>;
      case "i":
        return <em key={i}>{part.v}</em>;
      case "a":
        return (
          <a key={i} href={part.href} target="_blank" rel="noreferrer noopener">
            {part.v}
          </a>
        );
    }
  });
}
