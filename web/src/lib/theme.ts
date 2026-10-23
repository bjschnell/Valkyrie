import { useSyncExternalStore } from "react";

/**
 * The look: the default Dracula palette, or matrix (green), violet (pale purple),
 * valkyrie (the app icon's neon) or netrunner (an 80s hacker's neon HUD), all mono
 * with digital rain; valkyrie's is runes, netrunner's hex.
 */
export type Look = "default" | "matrix" | "violet" | "valkyrie" | "netrunner";
export const LOOKS: Look[] = ["default", "matrix", "violet", "valkyrie", "netrunner"];

/** Each look's name and a few words on it, for the settings page. */
export const LOOK_TEXT: Record<Look, { name: string; about: string }> = {
  default: { name: "Dracula", about: "The default: dark, soft colors" },
  matrix: { name: "Matrix", about: "Green on black, with digital rain" },
  violet: { name: "Violet", about: "Pale purple, with digital rain" },
  valkyrie: { name: "Valkyrie", about: "The icon's neon, with falling runes" },
  netrunner: { name: "Netrunner", about: "Black and neon, with falling hex" },
};

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

export function useLook(): Look {
  return useSyncExternalStore(
    (f) => {
      listeners.add(f);
      return () => listeners.delete(f);
    },
    () => look,
  );
}
