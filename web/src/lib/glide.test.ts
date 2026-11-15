import { describe, expect, it, vi } from "vitest";
import { glide, moved } from "./glide";

describe("moved", () => {
  it("is false for the same ids in the same order", () => {
    expect(moved([1, 2, 3], [1, 2, 3])).toBe(false);
    expect(moved([], [])).toBe(false);
  });
  it("sees a reorder, an arrival and a departure", () => {
    expect(moved([1, 2], [2, 1])).toBe(true);
    expect(moved([1], [1, 2])).toBe(true);
    expect(moved([1, 2], [1])).toBe(true);
  });
});

describe("glide", () => {
  it("just updates where there is no document or nothing moves", () => {
    const update = vi.fn();
    glide(update, true);
    glide(update, false);
    expect(update).toHaveBeenCalledTimes(2);
  });
});
