/** The code in a pairing link (`…/#pair=<code>`), or a bare code. */
export function parseCode(text: string): string | null {
  const m = /#pair=([A-Za-z0-9_-]+)\s*$/.exec(text) ?? /^\s*([A-Za-z0-9_-]{20,})\s*$/.exec(text);
  return m ? m[1] : null;
}
