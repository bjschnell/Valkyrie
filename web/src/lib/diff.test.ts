import { describe, expect, it } from "vitest";
import { diffRows, splitPath } from "./diff";

describe("diffRows", () => {
  it("numbers each side's lines", () => {
    const rows = diffRows("@@ -10,3 +10,3 @@ fn a() {\n keep\n-old\n+new\n same\n\\ No newline at end of file");
    expect(rows).toEqual([
      { kind: "hunk", text: "fn a() {" },
      { kind: "ctx", text: "keep", old: 10, new: 10 },
      { kind: "del", text: "old", old: 11 },
      { kind: "add", text: "new", new: 11 },
      { kind: "ctx", text: "same", old: 12, new: 12 },
      { kind: "note", text: "No newline at end of file" },
    ]);
  });

  it("names a bare hunk by its line", () => {
    expect(diffRows("@@ -0,0 +1,1 @@\n+hi")[0]).toEqual({ kind: "hunk", text: "line 1" });
    // A deleted file has no new lines; it's named by the old.
    expect(diffRows("@@ -1 +0,0 @@\n-gone")[0]).toEqual({ kind: "hunk", text: "line 1" });
  });

  it("splits a path", () => {
    expect(splitPath("src/lib/chat.ts")).toEqual({ name: "chat.ts", dir: "src/lib/" });
    expect(splitPath("README.md")).toEqual({ name: "README.md", dir: "" });
  });
});
