import { describe, expect, it } from "vitest";
import { rainFalls } from "./rain";

describe("rainFalls", () => {
  it("falls on a visible tab with nothing waiting", () => {
    expect(rainFalls(false, 0)).toBe(true);
  });
  it("stops while the tab is hidden", () => {
    expect(rainFalls(true, 0)).toBe(false);
  });
  it("stops while a card needs you", () => {
    expect(rainFalls(false, 1)).toBe(false);
    expect(rainFalls(true, 3)).toBe(false);
  });
});
