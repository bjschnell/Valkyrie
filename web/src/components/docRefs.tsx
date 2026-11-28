import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { docRefs } from "../lib/docs";
import { fileRefs, mentions } from "../lib/files";

/**
 * Opens a doc by its key beside the session (backlog #311). Only the session
 * view gives one, so doc mentions link there and stay text everywhere else.
 */
export const DocOpener = createContext<((key: string) => void) | null>(null);

/**
 * A session's markdown and text files (ADR-0021 item 9): a path mentioned in its
 * chat links only once the daemon says it passes the checks. Only the session
 * view gives one.
 */
export interface FileLinks {
  /** What was written, to the file it names; null when the daemon refused it. */
  passing: ReadonlyMap<string, string | null>;
  /** Asks the daemon about paths it hasn't been asked about yet. */
  ask: (paths: string[]) => void;
  open: (path: string) => void;
}
export const FileOpener = createContext<FileLinks | null>(null);

/** `text` with each doc it mentions ("doc 8 written", `valk doc read 8`, "doc #8"),
 * and each file of the session's that passed the checks, as a link that opens it
 * beside the session; plain text outside one. */
export function DocRefs({ text }: { text: string }): ReactNode {
  const open = useContext(DocOpener);
  const files = useContext(FileOpener);
  const hasFiles = files !== null;
  const found = useMemo(() => (hasFiles ? mentions(text) : []), [hasFiles, text]);
  const ask = files?.ask;
  useEffect(() => {
    if (ask && found.length) ask(found);
  }, [ask, found]);
  if (!open && !files) return text;
  const refs = open ? docRefs(text) : [{ t: "text" as const, v: text }];
  if (!refs.some((r) => r.t === "doc") && !found.some((f) => files?.passing.get(f))) return text;
  return refs.flatMap((r, i) =>
    r.t === "doc" ? (
      <button key={i} type="button" className="link doc-ref" title="Read it beside the session" onClick={() => open?.(r.key)}>
        {r.v}
      </button>
    ) : (
      fileRefs(r.v).map((f, j) => {
        const path = f.t === "file" ? files?.passing.get(f.v) : null;
        return path ? (
          <button key={`${i}.${j}`} type="button" className="link doc-ref" title="Read it beside the session" onClick={() => files?.open(path)}>
            {f.v}
          </button>
        ) : (
          f.v
        );
      })
    ),
  );
}
