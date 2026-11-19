import { describe, expect, it } from "vitest";
import type { WikiListing, WikiPage } from "../proto";
import { exploreLabel, staleCount, wikiSummary } from "./wiki";

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
