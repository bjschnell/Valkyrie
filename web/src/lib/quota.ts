// The plan's quota as the home screens show it (protocol 29); the TUI's is
// crates/valkyrie-tui/src/quota.rs, with the same names and thresholds.

import type { Quota } from "../proto";
import { age } from "./time";

/** Past this the reading says how old it is. */
const FRESH_MS = 5 * 60 * 1000;
/** From here a window's share is a warning, and from `FULL` it needs you. */
const LOW = 75;
const FULL = 90;

export type Level = "ok" | "low" | "full" | "reset";

export interface Window {
  label: string;
  used: number;
  level: Level;
  /** `resets in 2h`, while it hasn't. */
  resets: string | null;
}

export interface Reading {
  windows: Window[];
  /** `12m ago` once the reading is old, else null. */
  stale: string | null;
}

/** A window's short name: `5h`, `week`, `week · opus`, and Claude Code's own for the rest. */
export function label(name: string): string {
  if (name === "five_hour") return "5h";
  if (name === "seven_day") return "week";
  if (name.startsWith("seven_day_")) return `week · ${name.slice(10).replace(/_/g, " ")}`;
  return name.replace(/_/g, " ");
}

export function read(quota: Quota, nowMs: number): Reading {
  const atMs = quota.at * 1000;
  const old = nowMs - atMs > FRESH_MS;
  return {
    windows: quota.limits.map((l) => {
      const resetMs = l.resets_at === undefined ? null : l.resets_at * 1000;
      if (resetMs !== null && resetMs <= nowMs)
        return { label: label(l.name), used: 0, level: "reset", resets: null };
      return {
        label: label(l.name),
        used: l.used,
        level: l.used >= FULL ? "full" : l.used >= LOW ? "low" : "ok",
        // `age` counts from then to now; here it's now to the reset.
        resets: resetMs === null ? null : `resets in ${age(nowMs, resetMs)}`,
      };
    }),
    stale: old ? `${age(atMs, nowMs)} ago` : null,
  };
}
