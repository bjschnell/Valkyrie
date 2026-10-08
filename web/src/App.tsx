import { useEffect, useState } from "react";
import { Inbox } from "./components/Inbox";
import { Launcher } from "./components/Launcher";
import { Pair } from "./components/Pair";
import { ReviewView } from "./components/Review";
import { SessionView } from "./components/SessionView";
import { Toast } from "./components/Toast";
import { close, connect, open, useApp } from "./store";

type Route =
  | { page: "inbox" }
  | { page: "new" }
  | { page: "session"; id: number }
  | { page: "review"; id: number };

/** `#/s/12` is session 12, `#/s/12/review` its changes, `#/new` the launcher. */
function route(): Route {
  const m = /^#\/s\/(\d+)(\/review)?$/.exec(location.hash);
  if (m) return { page: m[2] ? "review" : "session", id: Number(m[1]) };
  if (location.hash === "#/new") return { page: "new" };
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
      {at.page === "session" && <SessionView id={at.id} />}
      {at.page === "review" && <ReviewView id={at.id} />}
      <Toast />
    </>
  );
}
