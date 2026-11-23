// What drove the last change: a key or a pointer (#198). Nothing a key caused
// animates: a page you reach with g i, a row Alt+J lands on, a list j scrolls.
// The root carries data-input="key" after a key and "pointer" after a tap or
// click, so CSS can drop transitions while the keyboard drives; script asks keyed().

type Root = { dataset: DOMStringMap };
type Target = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/** Watch keys and pointers on `on`, marking `root`. Returns the unhook. */
export function trackInput(on: Target = window, root: Root = document.documentElement): () => void {
  const mark = (to: "key" | "pointer") => () => {
    if (root.dataset.input !== to) root.dataset.input = to;
  };
  const key = mark("key");
  const pointer = mark("pointer");
  on.addEventListener("keydown", key, true);
  on.addEventListener("pointerdown", pointer, true);
  return () => {
    on.removeEventListener("keydown", key, true);
    on.removeEventListener("pointerdown", pointer, true);
  };
}

/** Whether the last thing you did was press a key. */
export function keyed(root: Root = document.documentElement): boolean {
  return root.dataset.input === "key";
}
