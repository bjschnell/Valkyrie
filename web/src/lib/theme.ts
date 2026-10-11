import { useSyncExternalStore } from "react";

/**
 * The look: the default Dracula palette, or matrix (green) or violet (pale purple),
 * both mono with digital rain.
 */
export type Look = "default" | "matrix" | "violet";
const LOOKS: Look[] = ["default", "matrix", "violet"];

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
    root.fx = "";
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
