import { describe, expect, it } from "vitest";
import type { RepoList } from "../proto";
import { addedRepo, isKnown, isScanned, repoChoices } from "./repos";

const list = (...paths: string[]): RepoList => ({
  repos: paths.map((path) => ({ path, label: path })),
  folders: [{ path: "/h/repos", label: "~/repos" }],
  hidden: [],
});

describe("repoChoices", () => {
  it("names repos by folder, sorted", () => {
    expect(repoChoices(list("/r/tool", "/r/app"))).toEqual([
      { repo: "/r/app", label: "app" },
      { repo: "/r/tool", label: "tool" },
    ]);
  });

  it("names two repos with one name by their paths", () => {
    expect(repoChoices(list("/a/app", "/b/app")).map((c) => c.label)).toEqual(["/a/app", "/b/app"]);
  });

  it("keeps the picked one offered", () => {
    expect(repoChoices(list("/r/app"), "/x/gone").map((c) => c.repo)).toEqual(["/r/app", "/x/gone"]);
    expect(repoChoices(null, "/r/app")).toHaveLength(1);
    expect(repoChoices(list("/r/app"), "/r/app")).toHaveLength(1);
  });

  it("knows scanned folders and known repos", () => {
    expect(isScanned(list(), "/h/repos")).toBe(true);
    expect(isKnown(list("/r/app"), "/r/app")).toBe(true);
    expect(isKnown(null, "/r/app")).toBe(false);
  });
});

describe("addedRepo", () => {
  it("finds the repo an add gave, though it reads otherwise", () => {
    const before = list("/r/app");
    const after: RepoList = { ...before, repos: [...before.repos, { path: "/r/main", label: "~/main", added: true }] };
    expect(addedRepo(before, after, "~/main-feat")).toBe("/r/main");
    // Added already: it reads as typed.
    expect(addedRepo(after, after, "~/main")).toBe("/r/main");
    expect(addedRepo(after, after, "~/nope")).toBeNull();
  });
});
