/**
 * A session's markdown and text files beside it (ADR-0021 item 9). The daemon
 * does every check; these only find what to ask it about.
 */

/** A run of text, or a mention of a markdown or text file by its path. */
export type FileRef = { t: "text"; v: string } | { t: "file"; v: string };

// A path ending in .md, .markdown or .txt: folders of word characters, dots,
// dashes and `~`, optionally from `/`. Not part of a URL or a longer word.
const MENTION = /(?<![\w./~:-])(?:~?\/)?(?:[\w.-]+\/)*[\w.-]*\w\.(?:md|markdown|txt)(?![\w/-])/gi;

/** `text` cut where it names a markdown or text file, so each can be checked. */
export function fileRefs(text: string): FileRef[] {
  const out: FileRef[] = [];
  let at = 0;
  for (const m of text.matchAll(MENTION)) {
    if (m[0].startsWith("~")) continue;
    if (m.index > at) out.push({ t: "text", v: text.slice(at, m.index) });
    out.push({ t: "file", v: m[0] });
    at = m.index + m[0].length;
  }
  if (at < text.length) out.push({ t: "text", v: text.slice(at) });
  return out;
}

/** The file mentions in `text`, each once. */
export function mentions(text: string): string[] {
  return [...new Set(fileRefs(text).flatMap((r) => (r.t === "file" ? [r.v] : [])))];
}

/** Whether `path` is plain text, shown preformatted rather than as markdown. */
export function isText(path: string): boolean {
  return /\.txt$/i.test(path);
}

/** Whether `path` is markdown. */
export function isMarkdown(path: string): boolean {
  return /\.(md|markdown)$/i.test(path);
}

/**
 * Where a link `href` in the markdown file `from` goes, as a path in the
 * repository: relative to `from`'s folder, or to the top level with a leading
 * `/`. Null for a web link or anything else, a link that climbs out of the
 * repository, or one that isn't to a markdown file. The daemon checks it again.
 */
export function linked(from: string, href: string): string | null {
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//")) return null;
  let path = href.split("#")[0].split("?")[0];
  try {
    path = decodeURIComponent(path);
  } catch {
    return null;
  }
  if (!path || path.includes("\\")) return null;
  const parts = path.startsWith("/") ? [] : from.split("/").slice(0, -1);
  for (const p of path.split("/")) {
    if (p === "" || p === ".") continue;
    if (p === "..") {
      if (!parts.length) return null;
      parts.pop();
    } else parts.push(p);
  }
  const out = parts.join("/");
  return out && isMarkdown(out) ? out : null;
}
