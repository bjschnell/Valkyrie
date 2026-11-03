import { describe, expect, it } from "vitest";
import { advice, mismatch } from "./protocol";

describe("mismatch", () => {
  it("is nothing when the daemon speaks the protocol this app was built with", () => {
    expect(mismatch({ protocol: 19, app_protocol: 19 })).toBeNull();
  });

  it("names both sides when they differ", () => {
    expect(mismatch({ protocol: 19, app_protocol: 18 })).toEqual({ daemon: 19, app: 18 });
    expect(mismatch({ protocol: 18, app_protocol: 19 })).toEqual({ daemon: 18, app: 19 });
  });

  it("counts a server too old to say as a mismatch", () => {
    expect(mismatch({ protocol: 19 })).toEqual({ daemon: 19, app: null });
  });
});

describe("advice", () => {
  it("upgrades a daemon older than the app", () => {
    expect(advice({ daemon: 18, app: 19 })).toMatch(/valk upgrade/);
  });

  it("restarts a valk web older than the daemon", () => {
    expect(advice({ daemon: 19, app: 18 })).toMatch(/restart valk web/);
    expect(advice({ daemon: 19, app: null })).toMatch(/restart valk web/);
  });
});
