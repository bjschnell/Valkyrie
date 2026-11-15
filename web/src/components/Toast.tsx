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
    const left = (toast.action ? ACTION_MS : SHOW_MS) - (Date.now() - toast.at);
    if (left <= 0) return setVisible(false);
    setVisible(true);
    const t = window.setTimeout(() => setVisible(false), left);
    return () => window.clearTimeout(t);
  }, [toast]);
  if (!toast) return null;
  const action = toast.action;
  return (
    <div className={`toast ${visible ? "show" : ""} ${action ? "with-action" : ""}`} role="status">
      {toast.text}
      {action && visible && (
        <button
          className="toast-action"
          onClick={() => {
            setVisible(false);
            action.run();
          }}
        >
          {action.label}
        </button>
      )}
    </div>
  );
}
