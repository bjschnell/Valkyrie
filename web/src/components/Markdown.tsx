import { useMemo, type ReactNode } from "react";
import { parseInline, parseMarkdown } from "../lib/chat";

/** Markdown as agents write it: chat replies, decision and skill bodies. */
export function Markdown({ text, className }: { text: string; className?: string }) {
  const parsed = useMemo(() => parseMarkdown(text), [text]);
  return (
    <div className={className ? `md ${className}` : "md"}>
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
              <List key={i} start={b.start}>
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
