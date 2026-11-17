import { afterEach, describe, expect, it, vi } from "vitest";
import { hostSignal, readHosts, validHost } from "./hosts";

afterEach(() => vi.unstubAllGlobals());

describe("readHosts", () => {
  it("reads what the desktop app said", () => {
    vi.stubGlobal("window", { __VALK_HOSTS__: { current: "box", hosts: ["box", "me@nas", 3] } });
    expect(readHosts()).toEqual({ current: "box", hosts: ["box", "me@nas"] });
  });

  it("is this computer when the window shows no host", () => {
    vi.stubGlobal("window", { __VALK_HOSTS__: { current: null, hosts: [] } });
    expect(readHosts()).toEqual({ current: null, hosts: [] });
  });

  it("is nothing outside the app", () => {
    vi.stubGlobal("window", {});
    expect(readHosts()).toBeNull();
    vi.stubGlobal("window", { __VALK_HOSTS__: "box" });
    expect(readHosts()).toBeNull();
  });
});

describe("validHost", () => {
  it("takes what ssh reads as one destination", () => {
    for (const ok of ["box", "me@box", "box.tail1234.ts.net", "10.0.0.2", "my_box-2"]) expect(validHost(ok)).toBe(true);
    for (const bad of ["", "-oProxyCommand=sh", "box x", "box;id", "$(id)", "a".repeat(254)])
      expect(validHost(bad)).toBe(false);
  });
});

describe("hostSignal", () => {
  it("names the host the way the app reads it back", () => {
    expect(hostSignal(null)).toBe("valk-desktop://local");
    expect(hostSignal("me@box")).toBe("valk-desktop://host/me%40box");
  });
});
