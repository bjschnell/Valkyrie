/** What a development daemon (`valk daemon --dev`) takes in place of pairing; any
 * other daemon refuses it like a revoked token. */
export const DEV_TOKEN = "dev-token";

/** The code in a pairing link (`…/#pair=<code>`), a bare link code, or a short code
 * as typed (`KQ7-4M2`, `kq7 4m2`). */
export function parseCode(text: string): string | null {
  const m = /#pair=([A-Za-z0-9_-]+)\s*$/.exec(text) ?? /^\s*([A-Za-z0-9_-]{20,})\s*$/.exec(text);
  if (m) return m[1];
  const short = text.replace(/[\s-]/g, "");
  return /^[0-9A-Za-z]{6}$/.test(short) ? short.toUpperCase() : null;
}
