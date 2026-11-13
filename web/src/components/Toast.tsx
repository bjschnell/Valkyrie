import { useEffect, useState } from "react";
import { useApp } from "../store";

const SHOW_MS = 2_500;
/** Long enough to reach an Undo. */
const ACTION_MS = 8_000;

export function Toast() {
  const toast = useApp((s) => s.toast);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!toast) return;
    setVisible(true);
    const t = window.setTimeout(() => setVisible(false), toast.action ? ACTION_MS : SHOW_MS);
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
