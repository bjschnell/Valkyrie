// Cards that glide to their new place when an agent changes state (#186).
//
// The View Transitions API snapshots the page, runs the update, and animates
// every element that carries a view-transition-name from where it was to where
// it is. The names live in CSS under html[data-vt="glide"], so they exist only
// while a glide runs and page transitions never morph the cards.

import { flushSync } from "react-dom";

/** True when the ids are not the same list in the same order. */
export function moved(a: readonly number[], b: readonly number[]): boolean {
  return a.length !== b.length || a.some((id, i) => id !== b[i]);
}

/** Whether a glide can run now: the browser has it, motion is allowed, the tab
 * is showing, and there are cards on screen to move. */
function can(): boolean {
  return (
    typeof document !== "undefined" &&
    typeof document.startViewTransition === "function" &&
    document.visibilityState === "visible" &&
    !matchMedia("(prefers-reduced-motion: reduce)").matches &&
    document.querySelector("[data-glide]") !== null
  );
}

/** Run `update`; when `shifts` says cards will change place, animate them. */
export function glide(update: () => void, shifts: boolean): void {
  if (!shifts || !can()) return update();
  const root = document.documentElement;
  // A page transition already running owns the root; let it finish alone.
  if (root.dataset.vt && root.dataset.vt !== "glide") return update();
  root.dataset.vt = "glide";
  const run = document.startViewTransition(() => flushSync(update));
  const done = () => {
    if (root.dataset.vt === "glide") delete root.dataset.vt;
  };
  void run.finished.then(done, done);
}
