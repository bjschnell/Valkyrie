// Other machines, in the desktop app (#179). The app tells the page which hosts
// it knows and which one this window shows (`window.__VALK_HOSTS__`, set before
// the page's scripts and again when the list changes, with a `valk-hosts`
// event). The page asks for one by navigating to `valk-desktop://host/NAME`:
// the app hears that and opens the host's own window, through an SSH tunnel.

export interface Hosts {
  /** The host this window shows; `null` is this computer. */
  current: string | null;
  hosts: string[];
}

type WithHosts = Window & { __VALK_HOSTS__?: unknown };

/** What the desktop app said; `null` outside it, or from an app without hosts. */
export function readHosts(): Hosts | null {
  const said = (window as WithHosts).__VALK_HOSTS__;
  if (!said || typeof said !== "object") return null;
  const { current, hosts } = said as { current?: unknown; hosts?: unknown };
  if (!Array.isArray(hosts)) return null;
  return {
    current: typeof current === "string" ? current : null,
    hosts: hosts.filter((h): h is string => typeof h === "string"),
  };
}

/**
 * Whether `name` is a host the app takes: one word `ssh` reads as a destination
 * (`box`, `me@box`, an alias), as `Host::parse` checks again in the app.
 */
export function validHost(name: string): boolean {
  return /^[A-Za-z0-9._@:-]{1,253}$/.test(name) && !name.startsWith("-");
}

/** Where the page goes to ask for `host`'s window; `null` is this computer's. */
export function hostSignal(host: string | null): string {
  return host === null ? "valk-desktop://local" : `valk-desktop://host/${encodeURIComponent(host)}`;
}
