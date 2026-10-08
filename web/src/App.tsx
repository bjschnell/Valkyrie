import { useEffect, useState } from "react";
import { Inbox } from "./components/Inbox";
import { Pair } from "./components/Pair";
import { SessionView } from "./components/SessionView";
import { Toast } from "./components/Toast";
import { close, connect, open, useApp } from "./store";

/** `#/s/12` is session 12; anything else is the inbox. */
function route(): number | null {
  const m = /^#\/s\/(\d+)$/.exec(location.hash);
  return m ? Number(m[1]) : null;
}

export function App() {
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [sessionId, setSessionId] = useState(route);

  useEffect(() => {
    const onHash = () => setSessionId(route());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (token) connect();
  }, [token]);

  useEffect(() => {
    if (sessionId === null) void close();
    else void open(sessionId);
  }, [sessionId]);

  if (!token || status === "unpaired") return <Pair />;
  return (
    <>
      {sessionId === null ? <Inbox /> : <SessionView id={sessionId} />}
      <Toast />
    </>
  );
}
