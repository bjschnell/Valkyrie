import { describe, expect, it } from "vitest";
import { scopeName } from "./scope";

describe("scopeName", () => {
  it("names the user's habits, and a project by its directory", () => {
    expect(scopeName("~user")).toBe("your habits");
    expect(scopeName("/home/u/repos/app")).toBe("app");
  });
});
