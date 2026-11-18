// Scrolls started from JS. CSS scroll-behavior can't override an explicit
// `behavior: "smooth"`, so the reduced-motion rule in styles.css doesn't reach
// them; they ask here instead.

/** "auto" (a jump) when the device asks for less motion, else "smooth". */
export function scrollMotion(): ScrollBehavior {
  return typeof matchMedia === "function" &&
    matchMedia("(prefers-reduced-motion: reduce)").matches
    ? "auto"
    : "smooth";
}
