import { describe, expect, it } from "vitest";
import { Kept } from "./kept";

/** A loader that resolves when told, counting its calls. */
function gate<T>(value: T | null) {
  let calls = 0;
  let open!: () => void;
  const opened = new Promise<void>((r) => (open = r));
  const get = async () => {
    calls++;
    await opened;
    return value;
  };
  return { get, open: () => open(), calls: () => calls };
}

describe("Kept", () => {
  it("has nothing for a key it never loaded", () => {
    expect(new Kept().peek("docs")).toBeNull();
  });

  it("keeps what a load gave, for the next visit", async () => {
    const k = new Kept();
    await k.load("docs", async () => ["a", "b"]);
    expect(k.peek("docs")).toEqual(["a", "b"]);
  });

  it("asks the daemon once while a load for the same key is under way", async () => {
    const k = new Kept();
    const g = gate(1);
    const first = k.load("past", g.get);
    const second = k.load("past", g.get);
    g.open();
    expect(await first).toBe(1);
    expect(await second).toBe(1);
    expect(g.calls()).toBe(1);
  });

  it("asks again once the last load finished", async () => {
    const k = new Kept();
    let n = 0;
    await k.load("past", async () => ++n);
    await k.load("past", async () => ++n);
    expect(k.peek("past")).toBe(2);
  });

  it("keeps different keys apart", async () => {
    const k = new Kept();
    await k.load("digest:away:repo", async () => "by repo");
    await k.load("digest:away:agent", async () => "by agent");
    expect(k.peek("digest:away:repo")).toBe("by repo");
    expect(k.peek("digest:away:agent")).toBe("by agent");
  });

  it("keeps the last data when the daemon couldn't be asked", async () => {
    const k = new Kept();
    await k.load("memory", async () => "old");
    expect(await k.load("memory", async () => null)).toBeNull();
    expect(k.peek("memory")).toBe("old");
  });

  it("keeps the last data when a load throws, and can load again after", async () => {
    const k = new Kept();
    await k.load("supervisor-log", async () => "old");
    await expect(
      k.load("supervisor-log", async () => {
        throw new Error("not connected");
      }),
    ).rejects.toThrow("not connected");
    expect(k.peek("supervisor-log")).toBe("old");
    await k.load("supervisor-log", async () => "new");
    expect(k.peek("supervisor-log")).toBe("new");
  });
});
