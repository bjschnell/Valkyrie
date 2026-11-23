import { describe, expect, it } from "vitest";
import { keyed, trackInput } from "./input";

// Node's EventTarget can't remove a capture listener, so a small fake stands in.
const setup = () => {
  const heard = new Map<string, EventListener>();
  const on = {
    addEventListener: (type: string, fn: EventListener, capture?: boolean) => {
      expect(capture).toBe(true);
      heard.set(type, fn);
    },
    removeEventListener: (type: string, fn: EventListener, capture?: boolean) => {
      expect(capture).toBe(true);
      if (heard.get(type) === fn) heard.delete(type);
    },
  } as Parameters<typeof trackInput>[0];
  const root = { dataset: {} as DOMStringMap };
  const fire = (type: string) => heard.get(type)?.(new Event(type));
  return { root, fire, heard, stop: trackInput(on, root) };
};

describe("trackInput", () => {
  it("starts as neither, so pointer behaviour (motion) is the default", () => {
    const { root } = setup();
    expect(keyed(root)).toBe(false);
  });
  it("marks a key, then a pointer", () => {
    const { root, fire } = setup();
    fire("keydown");
    expect(root.dataset.input).toBe("key");
    expect(keyed(root)).toBe(true);
    fire("pointerdown");
    expect(root.dataset.input).toBe("pointer");
    expect(keyed(root)).toBe(false);
  });
  it("lets go of both listeners once unhooked", () => {
    const { heard, stop } = setup();
    stop();
    expect(heard.size).toBe(0);
  });
});
