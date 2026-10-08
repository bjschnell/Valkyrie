import { useEffect, useState } from "react";
import { useApp } from "../store";

const SHOW_MS = 2_500;

export function Toast() {
  const toast = useApp((s) => s.toast);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!toast) return;
    setVisible(true);
    const t = window.setTimeout(() => setVisible(false), SHOW_MS);
    return () => window.clearTimeout(t);
  }, [toast]);
  if (!toast) return null;
  return (
    <div className={`toast ${visible ? "show" : ""}`} role="status">
      {toast.text}
    </div>
  );
}
