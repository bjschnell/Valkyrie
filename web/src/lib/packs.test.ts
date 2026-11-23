import { describe, expect, it } from "vitest";
import { NAME, packCss, type Pack } from "./packs";

const pack = (name: string, vars: Record<string, string>): Pack => ({ name, about: "", vars, glyphs: null, base: null });

describe("packCss", () => {
  it("writes one rule per pack, scoped to its look", () => {
    const css = packCss([pack("neon", { "--page": "#07050d", "--glow": "180 107 255" })]);
    expect(css).toBe(':root[data-theme="neon"] {\n  --page: #07050d;\n  --glow: 180 107 255;\n}');
  });

  it("drops what could break out of the rule", () => {
    const css = packCss([
      pack("neon", { "--page": "#000; } body { color: red", "--x}": "1", "color": "red", "--ok": "1" }),
      pack('x"] { } :root[data-theme="y', { "--page": "#000" }),
    ]);
    expect(css).toBe(':root[data-theme="neon"] {\n  --ok: 1;\n}');
  });

  it("knows a pack's name", () => {
    expect(NAME.test("neon")).toBe(true);
    expect(NAME.test("my-look-2")).toBe(true);
    for (const bad of ["", "Neon", "-x", "a b", "../x"]) expect(NAME.test(bad)).toBe(false);
  });
});
