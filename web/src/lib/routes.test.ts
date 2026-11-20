import { describe, expect, it } from "vitest";
import { docHash, HASH, memoryShelf, moved } from "./routes";

describe("moved", () => {
  it("sends the old pages to their section's tab", () => {
    expect(moved("#/decisions")).toBe(HASH.memory);
    expect(moved("#/docs")).toBe(HASH.docs);
    expect(moved("#/wiki")).toBe(HASH.wiki);
    expect(moved("#/summary")).toBe(HASH.summary);
    expect(moved("#/digest")).toBe(HASH.summary);
    expect(moved("#/search")).toBe(HASH.search);
    expect(moved("#/supervisor")).toBe(HASH.supervisor);
  });

  it("keeps a doc's key when its link moves", () => {
    expect(moved("#/docs/p1a2b")).toBe("#/memory/docs/p1a2b");
    expect(moved("#/docs/p1a2b")).toBe(docHash("p1a2b"));
  });

  it("leaves pages that haven't moved alone", () => {
    for (const h of ["", "#/", "#/history", "#/backlog", "#/backlog/3", "#/s/4", "#/memory/docs", "#/settings"]) {
      expect(moved(h)).toBeNull();
    }
  });
});

describe("memoryShelf", () => {
  it("reads Memory's shelves, null for the section itself", () => {
    expect(memoryShelf(HASH.memory)).toBeNull();
    expect(memoryShelf("#/memory/decisions")).toBe("decisions");
    expect(memoryShelf(HASH.habits)).toBe("habits");
    expect(memoryShelf(HASH.notes)).toBe("notes");
    expect(memoryShelf(HASH.docs)).toBeUndefined();
    expect(memoryShelf("#/memory/skills/2")).toBeUndefined();
  });
});
