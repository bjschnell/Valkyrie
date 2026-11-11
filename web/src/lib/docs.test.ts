import { describe, expect, it } from "vitest";
import type { Doc } from "../proto";
import { body, grouped, groupOf, resolver, validKey } from "./docs";

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
