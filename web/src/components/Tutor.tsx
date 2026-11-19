import { useState } from "react";
import { startTutor, toast } from "../store";

/** Starts the tutor (backlog #226), or opens the one already teaching. */
export function TutorButton({ className = "ghost", label = "Learn Valkyrie: start the tutor" }: { className?: string; label?: string }) {
  const [busy, setBusy] = useState(false);
  const start = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const id = await startTutor();
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Couldn't start the tutor: ${(e as Error).message}`);
      setBusy(false);
    }
  };
  return (
    <button className={className} disabled={busy} onClick={() => void start()}>
      {busy ? "Starting…" : label}
    </button>
  );
}
