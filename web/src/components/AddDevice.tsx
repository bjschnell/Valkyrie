import { useEffect, useState } from "react";
import { api, getJson } from "../lib/api";
import { useApp } from "../store";
import { Device } from "./icons";

interface ShortCode {
  code: string;
  id: string;
  expires_unix: number;
}

type CodeState = { state: "open" } | { state: "paired"; device: string } | { state: "gone" };

/** How often the panel asks whether its code was used. */
const POLL_MS = 2000;

/** The topbar's Add a device button, and the short code it shows (ADR-0009). */
export function AddDeviceButton() {
  const token = useApp((s) => s.token);
  const [open, setOpen] = useState(false);
  const [short, setShort] = useState<ShortCode | null>(null);
  const [state, setState] = useState<CodeState | null>(null);
  const [error, setError] = useState("");
  const [, setTick] = useState(0);

  useEffect(() => {
    if (!open) return;
    const timer = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(timer);
  }, [open]);

  const make = async () => {
    if (!token) return;
    setError("");
    setState(null);
    try {
      setShort(await (await api(token, "/api/pair/code", {})).json());
    } catch (e) {
      setShort(null);
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    if (!open || !short || !token || state?.state === "paired" || state?.state === "gone") return;
    const timer = window.setInterval(() => {
      getJson<CodeState>(token, `/api/pair/code/${encodeURIComponent(short.id)}`).then(setState, () => {});
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [open, short, token, state]);

  const left = short ? Math.max(0, Math.round(short.expires_unix - Date.now() / 1000)) : 0;
  const expired = !!short && (left === 0 || state?.state === "gone");

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && (!short || expired || state?.state === "paired")) void make();
  };

  return (
    <div className="notify">
      <button className="icon-btn" aria-label="Add a device" title="Add a device" onClick={toggle}>
        <Device />
      </button>
      {open && (
        <>
          <div className="scrim" onClick={() => setOpen(false)} />
          <div className="menu panel" role="dialog" aria-label="Add a device">
            <div className="panel-title">Add a device</div>
            {state?.state === "paired" ? (
              <p className="panel-text">Paired {state.device}.</p>
            ) : error ? (
              <p className="panel-text pair-error">{error}</p>
            ) : short && !expired ? (
              <>
                <p className="panel-text">
                  On the new device, open <code>{location.host}</code> (or the installed app) and type this code:
                </p>
                <div className="device-code" aria-label={`Code ${short.code}`}>
                  {short.code}
                </div>
                <p className="panel-text device-code-note">
                  Works once, for {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}. That device then stays
                  paired until you revoke it.
                </p>
              </>
            ) : short ? (
              <p className="panel-text">That code has expired.</p>
            ) : (
              <p className="panel-text">Making a code…</p>
            )}
            <div className="panel-actions">
              {(expired || state?.state === "paired" || error) && (
                <button className="primary" onClick={() => void make()}>
                  New code
                </button>
              )}
              <button className="ghost" onClick={() => setOpen(false)}>
                Done
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
