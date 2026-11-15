import { useState, type ReactNode } from "react";
import { reconnect, useApp } from "../store";

/** What the daemon-facing errors say, in words a person can act on. */
function plain(why: string): string {
  if (why === "not connected" || why === "disconnected") return "Valkyrie isn't reachable right now.";
  if (why === "no reply") return "The daemon didn't answer in time.";
  return why;
}

/**
 * A load that failed: what, why, and a way out. Retry runs `onRetry` and shows
 * it working; if the load fails again the page puts this back.
 */
export function ErrorState({ what, why, onRetry }: { what: string; why?: string | null; onRetry: () => void | Promise<unknown> }) {
  const status = useApp((s) => s.status);
  const [busy, setBusy] = useState(false);
  const retry = async () => {
    if (busy) return;
    setBusy(true);
    try {
      if (status !== "open") reconnect();
      await onRetry();
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="state state-error" role="alert">
      <div className="state-title">Couldn't load {what}</div>
      {why && <div className="state-sub">{plain(why)}</div>}
      <div className="state-actions">
        <button className="primary" disabled={busy} onClick={() => void retry()}>
          {busy ? "Retrying…" : "Retry"}
        </button>
      </div>
    </div>
  );
}

/** Nothing here yet: says so, and offers the next thing to do. */
export function EmptyState({
  title,
  hint,
  action,
}: {
  title: string;
  hint?: ReactNode;
  action?: { label: string; run: () => void };
}) {
  return (
    <div className="state">
      <div className="state-title">{title}</div>
      {hint && <div className="state-sub">{hint}</div>}
      {action && (
        <div className="state-actions">
          <button className="ghost" onClick={action.run}>
            {action.label}
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Shown while the app can't reach the daemon, whatever page it's on. The pill
 * says it in a word; this says what it means and offers to try now.
 */
export function OfflineBanner() {
  const status = useApp((s) => s.status);
  if (status !== "offline" && status !== "reconnecting") return null;
  const off = status === "offline";
  return (
    <div className={`offline-banner ${off ? "is-offline" : ""}`} role="status">
      <span className="offline-text">
        {off ? "You're offline. Showing what was last loaded." : "Lost the daemon. Reconnecting…"}
      </span>
      <button className="toast-action" onClick={reconnect}>
        Retry now
      </button>
    </div>
  );
}
