import { describe, expect, it } from "vitest";
import { follow } from "./follow";

const at = (s: Partial<Parameters<typeof follow>[0]>) =>
  follow({ open: null, followed: null, queue: [], typing: false, ...s });

describe("follow", () => {
  it("opens the top waiting session on the bare Inbox", () => {
    expect(at({ queue: [4, 2] })).toEqual({ open: 4 });
  });

  it("leaves the bare Inbox alone when nothing waits", () => {
    expect(at({})).toBeNull();
  });

  it("stays on a followed session while it still waits", () => {
    expect(at({ open: 4, followed: 4, queue: [2, 4] })).toBeNull();
  });

  it("moves to the next waiting session once the followed one is answered", () => {
    expect(at({ open: 4, followed: 4, queue: [2, 7] })).toEqual({ open: 2 });
  });

  it("stays on the answered session when nothing else waits", () => {
    expect(at({ open: 4, followed: 4, queue: [] })).toBeNull();
  });

  it("never moves while you're typing", () => {
    expect(at({ open: 4, followed: 4, queue: [2], typing: true })).toBeNull();
  });

  it("never moves away from a session you picked", () => {
    expect(at({ open: 9, followed: 4, queue: [2] })).toBeNull();
    expect(at({ open: 9, followed: null, queue: [2] })).toBeNull();
  });
});
