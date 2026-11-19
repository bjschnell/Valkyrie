import { afterEach, describe, expect, it, vi } from "vitest";
import { scrollMotion } from "./motion";

const media = (reduce: boolean) =>
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: reduce && q.includes("reduce") }));

describe("scrollMotion", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("glides when motion is allowed", () => {
    media(false);
    expect(scrollMotion()).toBe("smooth");
  });
  it("jumps when the device asks for less motion", () => {
    media(true);
    expect(scrollMotion()).toBe("auto");
  });
  it("glides where there is no matchMedia", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(scrollMotion()).toBe("smooth");
  });
});
