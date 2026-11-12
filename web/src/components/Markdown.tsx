import { useMemo, type HTMLAttributes, type ReactNode } from "react";
import { parseInline, parseMarkdown } from "../lib/chat";
import { quoteOf } from "../lib/docs";

/** Where a `[[wikilink]]` goes: a doc's key, or null when no doc goes by that name. */
export type WikiResolver = (target: string) => string | null;

/**
 * Commenting on blocks (ADR-0021 §6): headings, paragraphs, list items and code
 * blocks can be picked, each by its quote, and show how many comments they have.
 */
export interface BlockComments {
  /** The quote of the block picked, if any. */
  picked: string | null;
  pick: (quote: string) => void;
  /** Comments per quote, closed ones left out. */
  counts: Map<string, number>;
}

/** What a commentable block gets: a tap or Enter picks it. A tap on a link or
 * checkbox inside it stays theirs. */
function commentable(c: BlockComments | undefined, text: string, base?: string): HTMLAttributes<HTMLElement> {
  if (!c) return base ? { className: base } : {};
  const quote = quoteOf(text);
  const n = c.counts.get(quote) ?? 0;
  const classes = [base, "md-c", c.picked === quote && "md-picked", n > 0 && "md-commented"].filter(Boolean);
  return {
    className: classes.join(" "),
    tabIndex: 0,
    ...(n > 0 ? { "data-comments": String(n) } : {}),
    onClick: (e) => {
      if ((e.target as HTMLElement).closest("a, input, button")) return;
      if (window.getSelection()?.toString()) return;
      c.pick(quote);
    },
    onKeyDown: (e) => {
      if (e.key === "Enter" && e.target === e.currentTarget) {
        e.preventDefault();
        c.pick(quote);
      }
    },
  };
}

/**
 * Markdown as agents write it: chat replies, decision and skill bodies, docs.
 * Everything is React text, so HTML in it shows as text; only http(s) links
 * open, and a `[[wikilink]]` only to a doc `wiki` knows (in docs; text elsewhere).
 */
export function Markdown({
  text,
  className,
  wiki,
  comments,
}: {
  text: string;
  className?: string;
  wiki?: WikiResolver;
  comments?: BlockComments;
}) {
  const parsed = useMemo(() => parseMarkdown(text), [text]);
  const inline = (s: string) => inlines(s, wiki);
  return (
    <div className={className ? `md ${className}` : "md"}>
      {parsed.map((b, i) => {
        switch (b.t) {
          case "p":
            return (
              <p key={i} {...commentable(comments, b.text)}>
                {inline(b.text)}
              </p>
            );
          case "h":
            return (
              <div key={i} {...commentable(comments, b.text, `md-h md-h${Math.min(b.level, 3)}`)}>
                {inline(b.text)}
              </div>
            );
          case "code":
            return (
              <pre key={i} {...commentable(comments, b.text, "md-code")}>
                {b.text}
              </pre>
            );
          case "table":
            return (
              <div key={i} className="md-table-wrap">
                <table className="md-grid">
                  <thead>
                    <tr>
                      {b.head.map((c, k) => (
                        <th key={k} style={b.align[k] ? { textAlign: b.align[k] } : undefined}>
                          {inline(c)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {b.rows.map((r, j) => (
                      <tr key={j}>
                        {r.map((c, k) => (
                          <td key={k} style={b.align[k] ? { textAlign: b.align[k] } : undefined}>
                            {inline(c)}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
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
              <List key={i} start={b.start} className={b.checks ? "md-tasks" : undefined}>
                {b.items.map((item, j) => {
                  const check = b.checks?.[j];
                  return (
                    <li key={j} {...commentable(comments, item, check == null ? undefined : check ? "md-task done" : "md-task")}>
                      {check != null && (
                        <input type="checkbox" checked={check} disabled readOnly aria-label={check ? "done" : "not done"} />
                      )}
                      {inline(item)}
                    </li>
                  );
                })}
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

function inlines(text: string, wiki?: WikiResolver): ReactNode[] {
  return parseInline(text).map((part, i) => {
    switch (part.t) {
      case "wiki": {
        const key = wiki?.(part.target) ?? null;
        if (key === null) return wiki ? <span key={i} className="md-wiki missing" title="No doc goes by that name">{part.v}</span> : `[[${part.v}]]`;
        return (
          <a key={i} className="md-wiki" href={`#/docs/${encodeURIComponent(key)}`}>
            {part.v}
          </a>
        );
      }
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
