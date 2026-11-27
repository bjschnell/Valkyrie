import { createContext, useContext, type ReactNode } from "react";
import { docRefs } from "../lib/docs";

/**
 * Opens a doc by its key beside the session (backlog #311). Only the session
 * view gives one, so doc mentions link there and stay text everywhere else.
 */
export const DocOpener = createContext<((key: string) => void) | null>(null);

/** `text` with each doc it mentions ("doc 8 written", `valk doc read 8`, "doc #8")
 * as a link that opens it beside the session; plain text outside one. */
export function DocRefs({ text }: { text: string }): ReactNode {
  const open = useContext(DocOpener);
  if (!open) return text;
  const refs = docRefs(text);
  if (!refs.some((r) => r.t === "doc")) return text;
  return refs.map((r, i) =>
    r.t === "text" ? (
      r.v
    ) : (
      <button key={i} type="button" className="link doc-ref" title="Read it beside the session" onClick={() => open(r.key)}>
        {r.v}
      </button>
    ),
  );
}
