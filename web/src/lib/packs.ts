// Theme packs (#191): looks that live on the machine `valk web` runs on.

/**
 * A theme pack on the machine `valk web` runs on, from `/api/themes`: its CSS
 * variables, its rain, and, when it has one, where its module is served from.
 * The module is mounted in place of the rain and told what the sessions are
 * doing, which only a pack's look may react to (see `PackLayer`).
 */
export interface Pack {
  name: string;
  about: string;
  vars: Record<string, string>;
  glyphs: string | null;
  base: string | null;
}

/** What a pack may be called: safe in a CSS selector and a URL. */
export const NAME = /^[a-z0-9][a-z0-9-]{0,39}$/;

/**
 * A value that stays inside its own declaration: none of `{ } ; < \ " '`, no
 * comment, and its brackets balanced, so one pack's slip can't swallow the next rule.
 */
export function cssValueSafe(v: string): boolean {
  if (/[{};<\\"']|\/\*/.test(v)) return false;
  const open = "([";
  const close = ")]";
  const stack: string[] = [];
  for (const c of v) {
    if (open.includes(c)) stack.push(close[open.indexOf(c)]);
    else if (close.includes(c) && stack.pop() !== c) return false;
  }
  return stack.length === 0;
}

/** The CSS for the packs' looks: one rule each, its variables as they came. */
export function packCss(list: Pack[]): string {
  const safe = cssValueSafe;
  return list
    .filter((p) => NAME.test(p.name))
    .map((p) => {
      const vars = Object.entries(p.vars)
        .filter(([k, v]) => /^--[a-z0-9-]+$/i.test(k) && safe(v))
        .map(([k, v]) => `  ${k}: ${v};`)
        .join("\n");
      return `:root[data-theme="${p.name}"] {\n${vars}\n}`;
    })
    .join("\n");
}
