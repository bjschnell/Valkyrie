import { describe, expect, it } from "vitest";
import { sessionDock } from "./sessionDock";

describe("session dock (#268)", () => {
  it("leaves the whole pane to the terminal at desktop width", () => {
    // Clicked into, the terminal takes the keyboard itself: the message box,
    // its mic and the choice buttons would only shrink it.
    expect(sessionDock("terminal", true)).toEqual({ choices: false, keybar: false, composer: false });
  });

  it("keeps the message box and keys on a phone, the terminal's only way in", () => {
    expect(sessionDock("terminal", false)).toEqual({ choices: true, keybar: true, composer: true });
  });

  it("keeps the message box in the chat", () => {
    expect(sessionDock("chat", true)).toEqual({ choices: true, keybar: true, composer: true });
    expect(sessionDock("chat", false)).toEqual({ choices: true, keybar: true, composer: true });
  });
});
