import { useSyncExternalStore } from "react";
import type { TermColors } from "../proto";
import { NAME, packCss, type Pack } from "./packs";

export type { Pack };

/**
 * The look: the default Dracula palette, the TUI's other editor palettes (some
 * light), or the neon ones (matrix, violet, valkyrie, netrunner, cyberpunk), which
 * are mono with digital rain; runes fall in valkyrie and cyberpunk, hex in netrunner.
 * Or a theme pack's, by its name: a look that lives on the machine `valk web`
 * runs on (see `Pack`).
 */
export type Look = string;
type Builtin =
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
export const hasRain = (look: Look): boolean => FX.includes(look) || !!packOf(look)?.glyphs;

/** Each built-in look's name and a few words on it, for the settings page. */
const BUILTIN_TEXT: Record<Builtin, { name: string; about: string }> = {
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

/** Every look to pick from: the built-ins, then this machine's packs. */
export function lookList(): Look[] {
  return [...LOOKS, ...packs.map((p) => p.name)];
}

/** A look's name and a few words on it; a pack goes by its folder's name. */
export function lookText(look: Look): { name: string; about: string } {
  if (look in BUILTIN_TEXT) return BUILTIN_TEXT[look as Builtin];
  const pack = packOf(look);
  return { name: pack ? pack.name[0].toUpperCase() + pack.name.slice(1) : look, about: pack?.about ?? "" };
}

const KEY = "valk.look";
const listeners = new Set<() => void>();

let packs: Pack[] = [];
export const packOf = (look: Look): Pack | undefined => packs.find((p) => p.name === look);

function saved(): Look {
  try {
    const v = localStorage.getItem(KEY);
    // A pack's name is kept even before the packs load: they come with the token.
    return v && (LOOKS.includes(v) || NAME.test(v)) ? v : "default";
  } catch {
    return "default";
  }
}

let look = saved();
let version = 0;
const notify = () => {
  version++;
  listeners.forEach((f) => f());
};

/** The look as drawn: a pack's name stands for the default until its pack arrives. */
const shown = (): Look => (LOOKS.includes(look) || packOf(look) ? look : "default");

const apply = () => {
  const root = document.documentElement.dataset;
  const now = shown();
  if (now === "default") {
    delete root.theme;
    delete root.fx;
  } else {
    root.theme = now;
    if (hasRain(now)) root.fx = "";
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
  notify();
}

const subscribe = (f: () => void) => {
  listeners.add(f);
  return () => listeners.delete(f);
};

/** The look as drawn now. */
export function useLook(): Look {
  return useSyncExternalStore(subscribe, shown);
}

/** Bumps when the packs or the look change, for lists of looks. */
export function useLooksVersion(): number {
  return useSyncExternalStore(subscribe, () => version);
}

/** Takes this machine's packs (from `/api/themes`); one named like a built-in look is passed over. */
export function setPacks(list: Pack[]): void {
  packs = list.filter((p) => NAME.test(p.name) && !LOOKS.includes(p.name));
  let style = document.getElementById("valk-packs");
  if (!style) {
    style = document.createElement("style");
    style.id = "valk-packs";
    document.head.appendChild(style);
  }
  style.textContent = packCss(packs);
  apply();
  notify();
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
