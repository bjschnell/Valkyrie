import { useEffect, useState } from "react";
import { HOLD_MS, useApp } from "../store";

const SHOW_MS = 2_500;
/** Long enough to reach an Undo; a held action is sent as it goes. */
const ACTION_MS = HOLD_MS;

export function Toast() {
  const toast = useApp((s) => s.toast);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!toast) return;
    // Timed from when it was raised, not from this mount: a page change remounts
    // the toast, and a held action's Undo mustn't come back after it was sent.
    const lasts = toast.ms ?? (toast.action ? ACTION_MS : SHOW_MS);
    const left = lasts - (Date.now() - toast.at);
    if (left <= 0) return setVisible(false);
    setVisible(true);
    const t = window.setTimeout(() => setVisible(false), left);
    return () => window.clearTimeout(t);
  }, [toast]);
  if (!toast) return null;
  const actions = toast.actions ?? (toast.action ? [toast.action] : []);
  const open = toast.open;
  const live = actions.length > 0 || !!open;
  return (
    <div className={`toast ${visible ? "show" : ""} ${live ? "with-action" : ""}`} role="status">
      {open ? (
        <button
          className="toast-text"
          onClick={() => {
            setVisible(false);
            open();
          }}
        >
          {toast.text}
        </button>
      ) : (
        toast.text
      )}
      {visible &&
        actions.map((a) => (
          <button
            key={a.label}
            className="toast-action"
            onClick={() => {
              setVisible(false);
              a.run();
            }}
          >
            {a.label}
          </button>
        ))}
    </div>
  );
}
