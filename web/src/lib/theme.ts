import { useSyncExternalStore } from "react";
import type { TermColors } from "../proto";

/**
 * The look: the default Dracula palette, the TUI's other editor palettes (some
 * light), or the neon ones (matrix, violet, valkyrie, netrunner, cyberpunk), which
 * are mono with digital rain; runes fall in valkyrie and cyberpunk, hex in netrunner.
 */
export type Look =
  | "default"
  | "blackout"
  | "catppuccin"
  | "nord"
  | "gruvbox"
  | "tokyonight"
  | "solarized"
  | "onedark"
  | "rosepine"
  | "monokai"
  | "solarizedlight"
  | "catppuccinlatte"
  | "gruvboxlight"
  | "cyberpunk"
  | "matrix"
  | "violet"
  | "valkyrie"
  | "netrunner";
export const LOOKS: Look[] = [
  "default",
  "blackout",
  "catppuccin",
  "nord",
  "gruvbox",
  "tokyonight",
  "solarized",
  "onedark",
  "rosepine",
  "monokai",
  "solarizedlight",
  "catppuccinlatte",
  "gruvboxlight",
  "cyberpunk",
  "matrix",
  "violet",
  "valkyrie",
  "netrunner",
];

/** The looks with digital rain, scanlines and glitch (see `[data-fx]` in styles.css). */
const FX: Look[] = ["cyberpunk", "matrix", "violet", "valkyrie", "netrunner"];
export const hasRain = (look: Look): boolean => FX.includes(look);

/** Each look's name and a few words on it, for the settings page. */
export const LOOK_TEXT: Record<Look, { name: string; about: string }> = {
  default: { name: "Dracula", about: "The default: dark, soft colors" },
  blackout: { name: "Blackout", about: "Pure black with pastel neon" },
  catppuccin: { name: "Catppuccin", about: "Mocha: dark, warm pastels" },
  nord: { name: "Nord", about: "Cool, muted blues" },
  gruvbox: { name: "Gruvbox", about: "Dark, warm retro colors" },
  tokyonight: { name: "Tokyo Night", about: "Deep blue with soft neon" },
  solarized: { name: "Solarized Dark", about: "Teal-black, easy on the eyes" },
  onedark: { name: "One Dark", about: "Atom's dark theme" },
  rosepine: { name: "Rosé Pine", about: "Dusky purple, rose and gold" },
  monokai: { name: "Monokai", about: "Dark olive with vivid colors" },
  solarizedlight: { name: "Solarized Light", about: "Light: cream paper, low glare" },
  catppuccinlatte: { name: "Catppuccin Latte", about: "Light: soft pastels" },
  gruvboxlight: { name: "Gruvbox Light", about: "Light: warm retro colors" },
  cyberpunk: { name: "Cyberpunk", about: "Neon on navy, with falling runes" },
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
    if (hasRain(look)) root.fx = "";
    else delete root.fx;
  }
  // The browser's bar takes the page's color.
  const page = getComputedStyle(document.documentElement).getPropertyValue("--page").trim();
  if (page) document.querySelector('meta[name="theme-color"]')?.setAttribute("content", page);
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

const hexRgb = (c: string): [number, number, number] | null => {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(c.trim());
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : null;
};

/**
 * The colors a session's terminal is drawn with right now, for programs that ask
 * the terminal its background (OSC 11): a light look must not be told it is dark.
 */
export function termColors(): TermColors | null {
  const css = getComputedStyle(document.documentElement);
  const get = (name: string) => hexRgb(css.getPropertyValue(name));
  const fg = get("--term-fg");
  const bg = get("--term-bg");
  const ansi = Array.from({ length: 16 }, (_, i) => get(`--ansi-${i}`));
  if (!fg || !bg || ansi.some((c) => !c)) return null;
  return { fg, bg, ansi: ansi as [number, number, number][] };
}
