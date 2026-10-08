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

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
