// Which protocol this app speaks comes from the `valk web` that serves it
// (`app_protocol` on the hello reply), not a copy kept here: the server embeds
// this app, so its own build is the app's.

/** Both sides of a protocol disagreement; `app` is null when the server predates saying. */
export interface Mismatch {
  daemon: number;
  app: number | null;
}

export function mismatch(hello: { protocol: number; app_protocol?: number }): Mismatch | null {
  if (hello.app_protocol === hello.protocol) return null;
  return { daemon: hello.protocol, app: hello.app_protocol ?? null };
}

/** What fixes it: the older side moves to the newer build. */
export function advice(m: Mismatch): string {
  if (m.app !== null && m.app > m.daemon) {
    return "The daemon is older than this app: run valk upgrade --local on the machine running Valkyrie.";
  }
  return "valk web is older than the daemon: restart valk web (systemctl --user restart valk-web).";
}
