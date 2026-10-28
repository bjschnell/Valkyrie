import { useEffect, useState } from "react";
import { Backlog, ItemEditor } from "./components/Backlog";
import { Inbox } from "./components/Inbox";
import { Launcher } from "./components/Launcher";
import { Pair } from "./components/Pair";
import { ReviewView } from "./components/Review";
import { SessionView } from "./components/SessionView";
import { Settings } from "./components/Settings";
import { Toast } from "./components/Toast";
import { close, connect, open, useApp } from "./store";

type Route =
  | { page: "inbox" }
  | { page: "new" }
  | { page: "settings" }
  | { page: "backlog" }
  | { page: "item"; id: number | null }
  | { page: "session"; id: number }
  | { page: "review"; id: number };

/**
 * `#/s/12` is session 12, `#/s/12/review` its changes, `#/new` the launcher,
 * `#/settings` settings, `#/backlog` the backlog, `#/backlog/new` and `#/backlog/3`
 * writing an item down and changing item 3.
 */
function route(): Route {
  const m = /^#\/s\/(\d+)(\/review)?$/.exec(location.hash);
  if (m) return { page: m[2] ? "review" : "session", id: Number(m[1]) };
  if (location.hash === "#/new") return { page: "new" };
  if (location.hash === "#/settings") return { page: "settings" };
  if (location.hash === "#/backlog") return { page: "backlog" };
  const b = /^#\/backlog\/(new|\d+)$/.exec(location.hash);
  if (b) return { page: "item", id: b[1] === "new" ? null : Number(b[1]) };
  return { page: "inbox" };
}

export function App() {
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [at, setAt] = useState(route);

  useEffect(() => {
    const onHash = () => setAt(route());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (token) connect();
  }, [token]);

  // A session's page and its review both keep it open (its screen has the prompt).
  const openId = at.page === "session" || at.page === "review" ? at.id : null;
  useEffect(() => {
    if (openId === null) void close();
    else void open(openId);
  }, [openId]);

  if (!token || status === "unpaired") return <Pair />;
  return (
    <>
      {at.page === "inbox" && <Inbox />}
      {at.page === "new" && <Launcher />}
      {at.page === "settings" && <Settings />}
      {at.page === "backlog" && <Backlog />}
      {at.page === "item" && <ItemEditor key={at.id ?? "new"} id={at.id} />}
      {at.page === "session" && <SessionView id={at.id} />}
      {at.page === "review" && <ReviewView id={at.id} />}
      <Toast />
    </>
  );
}
