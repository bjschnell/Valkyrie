import { useSyncExternalStore } from "react";

/**
 * The look: the default Dracula palette, or matrix (green), violet (pale purple) or
 * valkyrie (the app icon's neon), all mono with digital rain; valkyrie's is runes.
 * Or netrunner, a quiet heads-up display after Cyberpunk 2077, with no rain.
 */
export type Look = "default" | "matrix" | "violet" | "valkyrie" | "netrunner";
const LOOKS: Look[] = ["default", "matrix", "violet", "valkyrie", "netrunner"];

/** Whether the look has digital rain and the effects that go with it. */
export const hasRain = (l: Look): boolean => l !== "default" && l !== "netrunner";

const KEY = "valk.look";
const listeners = new Set<() => void>();

function saved(): Look {
  try {
    const v = localStorage.getItem(KEY) as Look;
    return LOOKS.includes(v) ? v : "default";
  } catch {
    return "default";
  }
}

let look = saved();
const apply = () => {
  const root = document.documentElement.dataset;
  if (look === "default") {
    delete root.theme;
    delete root.fx;
  } else {
    root.theme = look;
    if (hasRain(look)) root.fx = "";
    else delete root.fx;
  }
};
apply();

export function setLook(next: Look): void {
  look = next;
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // Private mode: it lasts until the page closes.
  }
  apply();
  listeners.forEach((f) => f());
}

/** The one after `l`, for a button that cycles through them. */
export function nextLook(l: Look): Look {
  return LOOKS[(LOOKS.indexOf(l) + 1) % LOOKS.length];
}

export function useLook(): Look {
  return useSyncExternalStore(
    (f) => {
      listeners.add(f);
      return () => listeners.delete(f);
    },
    () => look,
  );
}
