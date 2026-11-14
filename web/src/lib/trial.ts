// #104 trial: the UI spikes under evaluation, each switchable in Settings so they
// can be compared against the current app. Removed once a direction is picked.

import { useSyncExternalStore } from "react";

export type Spike = "nav" | "cache" | "motion";
export const SPIKES: readonly Spike[] = ["nav", "cache", "motion"];

export const SPIKE_TEXT: Record<Spike, { name: string; about: string }> = {
  nav: { name: "Navigation", about: "Tab bar on a phone, a rail on a desktop" },
  cache: { name: "Instant pages", about: "Show the last data at once, refresh behind it" },
  motion: { name: "Motion", about: "Page transitions, hover and press feedback" },
};

const KEY = "valk.trial";
const listeners = new Set<() => void>();

function saved(): Set<Spike> {
  try {
    const v = localStorage.getItem(KEY);
    if (v === null) return new Set(SPIKES);
    return new Set(v.split(",").filter((s): s is Spike => (SPIKES as readonly string[]).includes(s)));
  } catch {
    return new Set(SPIKES);
  }
}

let on = saved();
const apply = () => {
  const root = document.documentElement.dataset;
  for (const s of SPIKES) {
    if (on.has(s)) root[`trial${s[0].toUpperCase()}${s.slice(1)}`] = "";
    else delete root[`trial${s[0].toUpperCase()}${s.slice(1)}`];
  }
};
apply();

export function trialOn(s: Spike): boolean {
  return on.has(s);
}

export function setSpike(s: Spike, value: boolean): void {
  on = new Set(on);
  if (value) on.add(s);
  else on.delete(s);
  try {
    localStorage.setItem(KEY, [...on].join(","));
  } catch {
    // Private mode: it lasts until the page closes.
  }
  apply();
  listeners.forEach((l) => l());
}

export function useSpike(s: Spike): boolean {
  return useSyncExternalStore(
    (changed) => {
      listeners.add(changed);
      return () => listeners.delete(changed);
    },
    () => on.has(s),
  );
}
