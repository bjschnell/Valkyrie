import { afterEach, describe, expect, it, vi } from "vitest";
import { inDesktopApp, support } from "./push";

afterEach(() => vi.unstubAllGlobals());

describe("support", () => {
  it("leaves notifications to the desktop app inside its window", () => {
    // A webview that could take web push all the same.
    vi.stubGlobal("window", { __VALK_DESKTOP__: true, isSecureContext: true, PushManager: {}, Notification: {} });
    expect(inDesktopApp()).toBe(true);
    expect(support()).toBe("desktop-app");
  });

  it("is a browser's own answer anywhere else", () => {
    vi.stubGlobal("window", { isSecureContext: false });
    expect(inDesktopApp()).toBe(false);
    expect(support()).toBe("needs-https");
  });
});
