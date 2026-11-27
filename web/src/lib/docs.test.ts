import { describe, expect, it } from "vitest";
import type { Doc } from "../proto";
import { body, countByQuote, docRefs, grouped, groupOf, onlyIn, quoteOf, resolver, validKey } from "./docs";

const doc = (path: string, key: string, title = ""): Doc => ({
  id: /^\d+$/.test(key) ? Number(key) : 0,
  kind: "note",
  title: title || path,
  path,
  key,
  by: "claude",
  created: "",
});

describe("docs", () => {
  it("lists agent-level docs first, then each repository's", () => {
    expect(groupOf("2026-11-09-plan.md")).toBe("");
    expect(groupOf("repos/valkyrie/x.md")).toBe("valkyrie");
    expect(groupOf("journal/x.md")).toBe("journal");
    const docs = [doc("repos/zeta/a.md", "1"), doc("top.md", "2"), doc("repos/alpha/b.md", "3"), doc("repos/zeta/c.md", "4")];
    expect(grouped(docs).map(([g, ds]) => [g, ds.map((d) => d.key)])).toEqual([
      ["", ["2"]],
      ["alpha", ["3"]],
      ["zeta", ["1", "4"]],
    ]);
  });

  it("drops the front matter", () => {
    expect(body("---\nvalk-id: 1\ntitle: x\n---\n\n# Hi\n")).toBe("# Hi\n");
    expect(body("no front matter")).toBe("no front matter");
    expect(body("---\nunclosed")).toBe("---\nunclosed");
  });

  it("resolves wikilinks only to listed docs, by path, file name or title", () => {
    const go = resolver([
      doc("repos/valkyrie/2026-11-09-docs-plan.md", "7", "Docs plan"),
      doc("notes.md", "p0123456789abcdef", "notes"),
    ]);
    expect(go("Docs plan")).toBe("7");
    expect(go("docs plan#Steps")).toBe("7");
    expect(go("2026-11-09-docs-plan")).toBe("7");
    expect(go("repos/valkyrie/2026-11-09-docs-plan.md")).toBe("7");
    expect(go("notes")).toBe("p0123456789abcdef");
    expect(go("../../.ssh/id_rsa")).toBeNull();
    expect(go("nothing")).toBeNull();
  });

  it("takes only keys a list could give out", () => {
    expect(validKey("12")).toBe(true);
    expect(validKey("p0123456789abcdef")).toBe(true);
    for (const bad of ["", "notes.md", "../x", "p123", "1/2", "%2e%2e"]) expect(validKey(bad)).toBe(false);
  });
});

describe("quoteOf", () => {
  it("keeps a block on one line, as the daemon does", () => {
    expect(quoteOf("Ship it\n  on **Friday**.\n")).toBe("Ship it on **Friday**.");
    expect(quoteOf("a‮b\u0007c")).toBe("a b c");
  });

  it("cuts a long block at 500 characters", () => {
    const q = quoteOf("é".repeat(600));
    expect([...q].length).toBe(501);
    expect(q.endsWith("…")).toBe(true);
  });
});

describe("countByQuote", () => {
  it("counts comments per block, not the whole doc's or closed ones", () => {
    const counts = countByQuote([
      { quote: "A", state: "open" },
      { quote: "A", state: "answered" },
      { quote: "A", state: "resolved" },
      { state: "open" },
      { quote: "B", state: "sent" },
    ]);
    expect([...counts.entries()]).toEqual([
      ["A", 2],
      ["B", 1],
    ]);
  });
});

describe("onlyIn", () => {
  it("marks the lines the other version lacks, never blank ones", () => {
    const mine = "# Plan\n\nShip Monday.\nThen rest.\n";
    const theirs = "# Plan\n\nShip Friday.\nThen rest.\n";
    expect([...onlyIn(mine, theirs)]).toEqual([2]);
    expect([...onlyIn(theirs, mine)]).toEqual([2]);
    expect([...onlyIn("a\n\n\n", "a")]).toEqual([]);
  });
});

describe("docRefs", () => {
  const docs = (text: string) => docRefs(text).flatMap((r) => (r.t === "doc" ? [[r.v, r.key]] : []));
  it("finds what writing, reading and editing a doc print", () => {
    expect(docs("doc 8 written: /home/me/docs/plan.md")).toEqual([["doc 8", "8"]]);
    expect(docs("run `valk doc read 12` then valk doc edit 12 --base ab -")).toEqual([
      ["valk doc read 12", "12"],
      ["valk doc edit 12", "12"],
    ]);
    expect(docs("Wrote it up as doc #9.")).toEqual([["doc #9", "9"]]);
  });
  it("keeps the rest of the text as it was", () => {
    const text = "see doc #3 and doc #4, done";
    expect(docRefs(text).map((r) => r.v).join("")).toBe(text);
  });
  it("leaves other numbers and words alone", () => {
    for (const text of ["doc 8", "docs 8 written", "the 2nd doc", "doc #0", "undoc #5", "valk doc list 3"]) {
      expect(docs(text), text).toEqual([]);
    }
    expect(docRefs("")).toEqual([]);
  });
  it("names a doc by its id without leading zeros", () => {
    expect(docs("doc #007")).toEqual([["doc #007", "7"]]);
  });
});
