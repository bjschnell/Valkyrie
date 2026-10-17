import { describe, expect, it } from "vitest";
import { pathBytes } from "./keys";

const text = (bytes: number[]) => new TextDecoder().decode(new Uint8Array(bytes));

describe("pathBytes", () => {
  it("pastes a path when the program takes bracketed paste", () => {
    expect(text(pathBytes("/s/pastes/1-a.png", { app_cursor: false, bracketed_paste: true, alt_screen: false }))).toBe(
      "\x1b[200~/s/pastes/1-a.png\x1b[201~",
    );
    expect(text(pathBytes("/s/pastes/1-a.png"))).toBe("/s/pastes/1-a.png");
  });

  it("quotes a path with a space", () => {
    expect(text(pathBytes("/home/A B/1.png"))).toBe('"/home/A B/1.png"');
  });
});
