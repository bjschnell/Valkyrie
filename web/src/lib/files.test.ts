import { describe, expect, it } from "vitest";
import { fileRefs, isText, linked, mentions } from "./files";

describe("fileRefs", () => {
  it("finds markdown and text paths in what an agent says", () => {
    expect(mentions("I wrote docs/plan.md and NOTES.txt, see README.markdown.")).toEqual([
      "docs/plan.md",
      "NOTES.txt",
      "README.markdown",
    ]);
  });
  it("takes absolute paths, and leaves the daemon to check them", () => {
    expect(mentions("Wrote /home/me/repo/DESIGN.md")).toEqual(["/home/me/repo/DESIGN.md"]);
  });
  it("leaves URLs, other kinds and home paths alone", () => {
    expect(mentions("https://x.io/a/README.md src/main.rs ~/notes.md foo.mdx a.md5")).toEqual([]);
  });
  it("keeps the text around a mention", () => {
    expect(fileRefs("see `a.md`.")).toEqual([
      { t: "text", v: "see `" },
      { t: "file", v: "a.md" },
      { t: "text", v: "`." },
    ]);
  });
});

describe("linked", () => {
  it("resolves a relative link against the file's folder", () => {
    expect(linked("docs/adr/0021.md", "0020.md")).toBe("docs/adr/0020.md");
    expect(linked("docs/adr/0021.md", "../usage.md#keys")).toBe("docs/usage.md");
    expect(linked("docs/a.md", "./b%20c.md")).toBe("docs/b c.md");
  });
  it("takes a leading slash as the top level", () => {
    expect(linked("docs/a.md", "/README.md")).toBe("README.md");
  });
  it("refuses web links, other kinds and climbing out", () => {
    expect(linked("a.md", "https://x.io/b.md")).toBeNull();
    expect(linked("a.md", "mailto:x@y")).toBeNull();
    expect(linked("a.md", "//x.io/b.md")).toBeNull();
    expect(linked("a.md", "src/main.rs")).toBeNull();
    expect(linked("docs/a.md", "../../etc/x.md")).toBeNull();
    expect(linked("a.md", "a\\b.md")).toBeNull();
    expect(linked("a.md", "#top")).toBeNull();
  });
});

describe("isText", () => {
  it("knows .txt", () => {
    expect(isText("a/NOTES.TXT")).toBe(true);
    expect(isText("a.md")).toBe(false);
  });
});
