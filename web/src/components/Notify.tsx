import { useState } from "react";
import type { ReactNode } from "react";
import type { PushState } from "../lib/push";
import { install, sendTestPush, turnOffPush, turnOnPush, useApp } from "../store";
import { Bell, BellOff } from "./icons";

const HINT_KEY = "valk.pushHint";

/** What each state says, and whether it can be turned on from here. */
const TEXT: Record<Exclude<PushState, "unknown">, ReactNode> = {
  on: "You'll get one when an agent needs you, finishes or stops, while you're away: no typing at any session for a minute and a half, and this app not open.",
  off: "Get a notification when an agent needs you, finishes or stops while you're away.",
  denied: "Notifications are blocked for Valkyrie. Allow them in your browser's or phone's settings, then come back.",
  "install-first": "On iPhone and iPad, notifications need Valkyrie on your Home Screen: tap Share, then Add to Home Screen, and open it from there.",
  "needs-https": (
    <>
      Notifications need HTTPS. On the computer, run <code>tailscale serve --bg 8790</code> and open the
      https:// address.
    </>
  ),
  unsupported: "This browser can't receive notifications.",
};

function TurnOn() {
  const busy = useApp((s) => s.pushBusy);
  return (
    <button className="primary" disabled={busy} onClick={() => void turnOnPush()}>
      {busy ? "Turning on…" : "Turn on"}
    </button>
  );
}

/** The topbar's bell, and the panel it opens. */
export function NotifyButton() {
  const push = useApp((s) => s.push);
  const canInstall = useApp((s) => s.canInstall);
  const [open, setOpen] = useState(false);
  if (push === "unknown") return null;
  return (
    <div className="notify">
      <button
        className={`icon-btn ${push === "on" ? "lit" : ""}`}
        aria-label="Notifications"
        onClick={() => setOpen((o) => !o)}
      >
        {push === "on" ? <Bell /> : <BellOff />}
      </button>
      {open && (
        <>
          <div className="scrim" onClick={() => setOpen(false)} />
          <div className="menu panel" role="dialog" aria-label="Notifications">
            <div className="panel-title">
              Notifications <span className={`panel-state ${push}`}>{push === "on" ? "on" : "off"}</span>
            </div>
            <p className="panel-text">{TEXT[push]}</p>
            <div className="panel-actions">
              {push === "off" && (
                <TurnOn />
              )}
              {push === "on" && (
                <>
                  <button className="primary" onClick={() => void sendTestPush()}>
                    Send a test
                  </button>
                  <button className="ghost" onClick={() => void turnOffPush()}>
                    Turn off
                  </button>
                </>
              )}
              {canInstall && (
                <button className="ghost" onClick={() => void install()}>
                  Install app
                </button>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/** Once, at the top of the inbox: notifications are the point of the phone app. */
export function NotifyHint() {
  const push = useApp((s) => s.push);
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(HINT_KEY) === "no";
    } catch {
      return false;
    }
  });
  if (dismissed || (push !== "off" && push !== "install-first")) return null;
  const dismiss = () => {
    try {
      localStorage.setItem(HINT_KEY, "no");
    } catch {
      /* shown again next time */
    }
    setDismissed(true);
  };
  return (
    <div className="hint">
      <div className="hint-icon">
        <Bell />
      </div>
      <div className="hint-body">
        <div className="hint-title">Hear from your agents</div>
        <div className="hint-text">{push === "off" ? "A notification when one needs you while you're away." : TEXT["install-first"]}</div>
        <div className="hint-actions">
          {push === "off" && (
            <TurnOn />
          )}
          <button className="ghost" onClick={dismiss}>
            Not now
          </button>
        </div>
      </div>
    </div>
  );
}
