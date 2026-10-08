// A file's patch as rows to show: each line with its old and new line numbers.

export type RowKind = "hunk" | "add" | "del" | "ctx" | "note";

export interface DiffRow {
  kind: RowKind;
  text: string;
  old?: number;
  new?: number;
}

export function diffRows(patch: string): DiffRow[] {
  const rows: DiffRow[] = [];
  let oldN = 0;
  let newN = 0;
  for (const line of patch.split("\n")) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hunk) {
      oldN = Number(hunk[1]);
      newN = Number(hunk[2]);
      rows.push({ kind: "hunk", text: hunk[3].trim() || `line ${newN || oldN}` });
    } else if (line.startsWith("+")) {
      rows.push({ kind: "add", text: line.slice(1), new: newN++ });
    } else if (line.startsWith("-")) {
      rows.push({ kind: "del", text: line.slice(1), old: oldN++ });
    } else if (line.startsWith("\\")) {
      rows.push({ kind: "note", text: line.slice(1).trim() });
    } else if (rows.length) {
      rows.push({ kind: "ctx", text: line.slice(1), old: oldN++, new: newN++ });
    }
  }
  return rows;
}

/** `src/lib/chat.ts` → name `chat.ts`, directory `src/lib/`. */
export function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf("/");
  return { name: path.slice(slash + 1), dir: path.slice(0, slash + 1) };
}
