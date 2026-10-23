import { useEffect, useState } from "react";
import { api, getJson } from "../lib/api";
import { useApp } from "../store";

interface ShortCode {
  code: string;
  id: string;
  expires_unix: number;
}

type CodeState = { state: "open" } | { state: "paired"; device: string } | { state: "gone" };

/** How often the panel asks whether its code was used. */
const POLL_MS = 2000;

/** Settings' Add a device section: a short code for another device to type (ADR-0009). */
export function AddDeviceSettings() {
  const token = useApp((s) => s.token);
  const [short, setShort] = useState<ShortCode | null>(null);
  const [state, setState] = useState<CodeState | null>(null);
  const [error, setError] = useState("");
  const [, setTick] = useState(0);

  // The countdown, while there's a code to count down.
  useEffect(() => {
    if (!short) return;
    const timer = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(timer);
  }, [short]);

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
    if (!short || !token || state?.state === "paired" || state?.state === "gone") return;
    const timer = window.setInterval(() => {
      getJson<CodeState>(token, `/api/pair/code/${encodeURIComponent(short.id)}`).then(setState, () => {});
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [short, token, state]);

  const left = short ? Math.max(0, Math.round(short.expires_unix - Date.now() / 1000)) : 0;
  const expired = !!short && (left === 0 || state?.state === "gone");

  return (
    <section>
      <h2 className="section-title">Add a device</h2>
      <div className="settings-card">
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
              The code works once, for {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}. That device then
              stays paired until you revoke it.
            </p>
          </>
        ) : short ? (
          <p className="panel-text">That code has expired.</p>
        ) : (
          <p className="panel-text">Pair a phone or another computer with a short code from here.</p>
        )}
        {(!short || expired || state?.state === "paired" || error) && (
          <div className="panel-actions">
            <button className="primary" onClick={() => void make()}>
              {short || error ? "New code" : "Show a code"}
            </button>
          </div>
        )}
      </div>
    </section>
  );
}
