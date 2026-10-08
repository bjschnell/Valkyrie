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
      { t: "list", ordered: false, items: ["one", "two continued"] },
      { t: "code", lang: "rust", text: "fn main() {}" },
      { t: "pre", text: "| a | b |\n|---|---|" },
      { t: "quote", text: "quoted" },
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
