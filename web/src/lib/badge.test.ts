import { afterEach, describe, expect, it, vi } from "vitest";
import { showNeeds, titleFor } from "./badge";

describe("titleFor", () => {
  it("puts the count in front, and nothing when none wait", () => {
    expect(titleFor(0)).toBe("Valkyrie");
    expect(titleFor(1)).toBe("(1) Valkyrie");
    expect(titleFor(12)).toBe("(12) Valkyrie");
  });
});

describe("showNeeds", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("sets the title and the badge, and clears both at zero", () => {
    const setAppBadge = vi.fn().mockResolvedValue(undefined);
    const clearAppBadge = vi.fn().mockResolvedValue(undefined);
    const doc = { title: "" };
    vi.stubGlobal("navigator", { setAppBadge, clearAppBadge });
    vi.stubGlobal("document", doc);
    showNeeds(3);
    expect(doc.title).toBe("(3) Valkyrie");
    expect(setAppBadge).toHaveBeenCalledWith(3);
    showNeeds(0);
    expect(doc.title).toBe("Valkyrie");
    expect(clearAppBadge).toHaveBeenCalled();
  });

  it("survives a browser with no badge API, or one that refuses", () => {
    const doc = { title: "" };
    vi.stubGlobal("document", doc);
    vi.stubGlobal("navigator", {});
    expect(() => showNeeds(2)).not.toThrow();
    expect(doc.title).toBe("(2) Valkyrie");
    vi.stubGlobal("navigator", { setAppBadge: () => Promise.reject(new Error("no")) });
    expect(() => showNeeds(2)).not.toThrow();
    vi.stubGlobal("navigator", {
      setAppBadge: () => {
        throw new Error("no");
      },
    });
    expect(() => showNeeds(2)).not.toThrow();
  });
});
