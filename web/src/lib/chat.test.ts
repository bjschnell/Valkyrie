import { describe, expect, it } from "vitest";
import type { ChatItem } from "../proto";
import { applyChat, blocks, fileLine, groupSummary, parseInline, parseMarkdown, type ToolItem } from "./chat";

const tool = (id: string, name = "Bash", status: ToolItem["status"] = "ok", file?: string): ToolItem => ({
  k: "tool",
  id,
  tool: name,
  summary: id,
  status,
  detail: file ? { file } : {},
});
const say = (id: string): ChatItem => ({ k: "say", id, text: id });

describe("applyChat", () => {
  it("adds items and replaces a finished tool call in place", () => {
    let chat = applyChat(null, { t: "chat", session: 1, reset: true, items: [say("a"), tool("t", "Bash", "running")] });
    chat = applyChat(chat, { t: "chat", session: 1, items: [tool("t"), say("b")] });
    expect(chat.items.map((i) => i.id)).toEqual(["a", "t", "b"]);
    expect((chat.items[1] as ToolItem).status).toBe("ok");
  });

  it("starts over on a reset or another session", () => {
    let chat = applyChat(null, { t: "chat", session: 1, reset: true, items: [say("a")] });
    chat = applyChat(chat, { t: "chat", session: 2, items: [say("x")] });
    expect(chat.session).toBe(2);
    expect(chat.items.map((i) => i.id)).toEqual(["x"]);
    chat = applyChat(chat, { t: "chat", session: 2, reset: true, items: [], missing: true });
    expect(chat.missing).toBe(true);
  });
});

describe("blocks", () => {
  it("groups runs of three or more tool calls", () => {
    const b = blocks([say("a"), tool("1"), tool("2"), say("b"), tool("3"), tool("4"), tool("5")]);
    expect(b.map((x) => (x.k === "tools" ? `group ${x.tools.length}` : x.item.id))).toEqual([
      "a",
      "1",
      "2",
      "b",
      "group 3",
    ]);
  });

  it("never folds an agent's question into a group", () => {
    const b = blocks([tool("1"), tool("2"), tool("3"), tool("q", "AskUserQuestion"), tool("4"), tool("5"), tool("6")]);
    expect(b.map((x) => (x.k === "tools" ? `group ${x.tools.length}` : x.item.id))).toEqual([
      "group 3",
      "q",
      "group 3",
    ]);
  });

  it("summarises a group", () => {
    expect(
      groupSummary([
        tool("1"),
        tool("2", "Bash", "error"),
        tool("3", "Edit", "ok", "a.rs"),
        tool("4", "Edit", "ok", "a.rs"),
        tool("5", "Read"),
        tool("6", "Bash", "denied"),
      ]),
    ).toBe("Ran 3 commands, edited 1 file, looked at 1 · 1 failed · 1 denied");
  });
});

describe("fileLine", () => {
  it("puts the file name and its change first", () => {
    const edit = { ...tool("e", "Edit", "ok", "crates/web/src/chat.rs"), summary: "crates/web/src/chat.rs (+3/-1)" };
    expect(fileLine(edit)).toEqual({ name: "chat.rs", dir: "crates/web/src/", added: 3, removed: 1, lines: null });
    const write = { ...tool("w", "Write", "ok", "notes.md"), summary: "notes.md (+12 lines)" };
    expect(fileLine(write)).toEqual({ name: "notes.md", dir: "", added: 0, removed: 0, lines: 12 });
    expect(fileLine(tool("b"))).toBeNull();
  });
});

describe("markdown", () => {
  it("reads the blocks agents write", () => {
    const md = [
      "## Done",
      "",
      "It works:",
      "- one",
      "- two",
      "  continued",
      "",
      "```rust",
      "fn main() {}",
      "```",
      "| a | b |",
      "|---|---|",
      "> quoted",
    ].join("\n");
    expect(parseMarkdown(md)).toEqual([
      { t: "h", level: 2, text: "Done" },
      { t: "p", text: "It works:" },
      { t: "list", ordered: false, items: ["one", "two\ncontinued"] },
      { t: "code", lang: "rust", text: "fn main() {}" },
      { t: "table", head: ["a", "b"], align: [null, null], rows: [] },
      { t: "quote", text: "quoted" },
    ]);
  });

  it("keeps numbered steps in one list when a command or blank line sits between them", () => {
    const md = [
      "1. Check the conflicts:",
      "`git merge-tree HEAD origin/main`",
      "",
      "2. Merge main.",
      "",
      "   If main rewrote the file, read it first.",
      "",
      "When it worked: done.",
    ].join("\n");
    expect(parseMarkdown(md)).toEqual([
      {
        t: "list",
        ordered: true,
        start: 1,
        items: ["Check the conflicts:\n`git merge-tree HEAD origin/main`", "Merge main.\n\nIf main rewrote the file, read it first."],
      },
      { t: "p", text: "When it worked: done." },
    ]);
  });

  it("starts an ordered list at its first number and stops a list at a heading", () => {
    expect(parseMarkdown("3. three\n4. four\n## Next\n- a")).toEqual([
      { t: "list", ordered: true, start: 3, items: ["three", "four"] },
      { t: "h", level: 2, text: "Next" },
      { t: "list", ordered: false, items: ["a"] },
    ]);
  });

  it("reads inline code, emphasis and links, and never links a file path", () => {
    expect(parseInline("Run `cargo test` **now**, see [docs](https://x.dev/a) or [lib.rs](src/lib.rs).")).toEqual([
      { t: "text", v: "Run " },
      { t: "code", v: "cargo test" },
      { t: "text", v: " " },
      { t: "b", v: "now" },
      { t: "text", v: ", see " },
      { t: "a", v: "docs", href: "https://x.dev/a" },
      { t: "text", v: " or " },
      { t: "text", v: "lib.rs" },
      { t: "text", v: "." },
    ]);
    expect(parseInline("at https://example.com/x.")).toEqual([
      { t: "text", v: "at " },
      { t: "a", v: "https://example.com/x", href: "https://example.com/x" },
      { t: "text", v: "." },
    ]);
    expect(parseInline("a * b * c")).toEqual([{ t: "text", v: "a * b * c" }]);
  });
});

describe("markdown for docs", () => {
  it("reads a table with its alignment, padding short rows", () => {
    const md = ["| name | n | note |", "|:--|--:|:-:|", "| `a\\|b` | 1 | **x** |", "| c | 2 |"].join("\n");
    expect(parseMarkdown(md)).toEqual([
      {
        t: "table",
        head: ["name", "n", "note"],
        align: ["left", "right", "center"],
        rows: [
          ["`a|b`", "1", "**x**"],
          ["c", "2", ""],
        ],
      },
    ]);
  });

  it("keeps pipe lines with no separator row as text", () => {
    expect(parseMarkdown("| just | pipes |\n| more |")).toEqual([{ t: "pre", text: "| just | pipes |\n| more |" }]);
  });

  it("reads task boxes, and leaves a list without any alone", () => {
    expect(parseMarkdown("- [x] done\n- [ ] todo\n- plain")).toEqual([
      { t: "list", ordered: false, items: ["done", "todo", "plain"], checks: [true, false, null] },
    ]);
    expect(parseMarkdown("- [link](https://x.dev)")).toEqual([
      { t: "list", ordered: false, items: ["[link](https://x.dev)"] },
    ]);
  });

  it("reads wikilinks, with a label after the bar", () => {
    expect(parseInline("see [[Docs plan]] and [[repos/valk/notes|the notes]]")).toEqual([
      { t: "text", v: "see " },
      { t: "wiki", v: "Docs plan", target: "Docs plan" },
      { t: "text", v: " and " },
      { t: "wiki", v: "the notes", target: "repos/valk/notes" },
    ]);
  });

  it("reads a long run of brackets in linear time", () => {
    for (const run of ["[".repeat(200_000), "[[a|".repeat(50_000), "[x".repeat(100_000)]) {
      const start = performance.now();
      parseInline(run);
      expect(performance.now() - start).toBeLessThan(500);
    }
  });

  it("opens only http(s) links; HTML and other schemes stay text", () => {
    expect(parseInline("[x](javascript:alert(1))")).toEqual([{ t: "text", v: "x" }, { t: "text", v: ")" }]);
    expect(parseInline("[x](data:text/html,hi)")).toEqual([{ t: "text", v: "x" }]);
    expect(parseInline('<img src=x onerror="alert(1)">')).toEqual([{ t: "text", v: '<img src=x onerror="alert(1)">' }]);
  });
});
