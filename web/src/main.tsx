import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { registerWorker } from "./lib/push";
import "./styles.css";

registerWorker();
// A tapped notification, when the app was already open: go to its session.
navigator.serviceWorker?.addEventListener("message", (e) => {
  if (e.data?.t === "open") location.hash = new URL(e.data.url, location.href).hash || "#/";
});

// iOS lays the keyboard over the page instead of shrinking it, so a message box at
// the bottom ends up under it. The visual viewport is what's left above the
// keyboard: the full-height pages take its height.
const view = window.visualViewport;
if (view) {
  let tallest = view.height;
  let width = view.width;
  const fit = () => {
    if (Math.abs(view.scale - 1) > 0.01) return; // pinch zoom, not the keyboard
    if (view.width !== width) tallest = 0; // turned: a new full height
    width = view.width;
    tallest = Math.max(tallest, view.height);
    const root = document.documentElement;
    root.style.setProperty("--app-height", `${view.height}px`);
    root.classList.toggle("keyboard", view.height < tallest * 0.8);
    if (view.offsetTop) window.scrollTo(0, 0);
  };
  view.addEventListener("resize", fit);
  view.addEventListener("scroll", fit);
  fit();
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
