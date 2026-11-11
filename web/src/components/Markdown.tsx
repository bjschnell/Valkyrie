import { useMemo, type ReactNode } from "react";
import { parseInline, parseMarkdown } from "../lib/chat";

/** Where a `[[wikilink]]` goes: a doc's key, or null when no doc goes by that name. */
export type WikiResolver = (target: string) => string | null;

/**
 * Markdown as agents write it: chat replies, decision and skill bodies, docs.
 * Everything is React text, so HTML in it shows as text; only http(s) links
 * open, and a `[[wikilink]]` only to a doc `wiki` knows (in docs; text elsewhere).
 */
export function Markdown({ text, className, wiki }: { text: string; className?: string; wiki?: WikiResolver }) {
  const parsed = useMemo(() => parseMarkdown(text), [text]);
  const inline = (s: string) => inlines(s, wiki);
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
                    <li key={j} className={check == null ? undefined : check ? "md-task done" : "md-task"}>
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
