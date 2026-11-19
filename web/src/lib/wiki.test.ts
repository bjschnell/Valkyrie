import { describe, expect, it } from "vitest";
import type { Item, SessionInfo, WikiListing, WikiPage } from "../proto";
import { exploreLabel, staleCount, wikiRepos, wikiSummary } from "./wiki";

const item = (project?: string) => ({ project }) as Item;
const session = (cwd: string) => ({ cwd }) as SessionInfo;
const page = (slug: string, stale?: string): WikiPage => ({
  slug,
  title: slug,
  summary: "",
  scope: [],
  updated: 0,
  stale,
  body: "",
});
const listing = (...pages: WikiPage[]): WikiListing => ({ root: "/r/app", pages });

describe("wikiRepos", () => {
  it("offers backlog repos and session dirs, but not a repo's worktrees or subfolders", () => {
    const got = wikiRepos(
      [item("/r/app"), item("/r/app"), item()],
      [session("/r/app-feat-x"), session("/r/app/web"), session("/r/tool"), session("/r/app")],
    );
    expect(got).toEqual([
      { repo: "/r/app", label: "app" },
      { repo: "/r/tool", label: "tool" },
    ]);
  });

  it("names two repos with one name by their paths", () => {
    const got = wikiRepos([item("/a/app"), item("/b/app")], []);
    expect(got.map((c) => c.label)).toEqual(["/a/app", "/b/app"]);
  });
});

describe("labels", () => {
  it("counts pages and stale ones", () => {
    expect(wikiSummary(listing(page("a")))).toBe("1 page");
    expect(wikiSummary(listing(page("a"), page("b", "300 lines changed")))).toBe("2 pages, 1 stale");
    expect(staleCount(null)).toBe(0);
  });

  it("says what an exploration does before it starts one", () => {
    expect(exploreLabel(listing(), false).button).toBe("Map this repo");
    expect(exploreLabel(listing(page("a")), false).button).toBe("Explore again");
    expect(exploreLabel(listing(page("a", "x"), page("b", "y")), true)).toEqual({
      button: "Refresh 2 stale",
      confirm: "Start an agent to rewrite 2 stale pages?",
    });
  });
});
