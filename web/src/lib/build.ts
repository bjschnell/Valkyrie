// Which build of the app this page is: the hashed files it loaded. `valk web` names
// its own build the same way (crates/valkyrie-web, `BUILD`) and sends it in `hello`,
// so a page left open across an upgrade can tell it's out of date.

/** The `/assets/` paths among `urls`, sorted, as one string; "" for a dev server. */
export function buildOf(urls: string[]): string {
  const files = urls
    .map((u) => new URL(u, "http://x").pathname)
    .filter((p) => p.startsWith("/assets/"));
  return [...new Set(files)].sort().join(" ");
}

/** This page's build, from its scripts and stylesheets. */
export function pageBuild(): string {
  const urls = [...document.querySelectorAll<HTMLScriptElement | HTMLLinkElement>("script[src], link[href]")].map(
    (el) => (el instanceof HTMLScriptElement ? el.src : el.href),
  );
  return buildOf(urls);
}
