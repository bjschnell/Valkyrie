import { useEffect, useState } from "react";
import { saveToken, useApp } from "../store";

/** A name for this device in `valk web devices`: the platform, roughly. */
function deviceName(): string {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android";
  if (/Mac/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows";
  return "browser";
}

/** The code from a scanned pairing link (`/#pair=<code>`). */
function linkCode(): string | null {
  const m = /^#pair=([A-Za-z0-9_-]+)$/.exec(location.hash);
  return m ? m[1] : null;
}

export function Pair() {
  const status = useApp((s) => s.status);
  const [code] = useState(linkCode);
  const [state, setState] = useState<"idle" | "pairing" | "failed">(code ? "pairing" : "idle");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!code) return;
    // The code is single-use: out of the address bar (and history) at once.
    history.replaceState(null, "", location.pathname);
    let live = true;
    (async () => {
      try {
        const res = await fetch("/api/pair", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ code, name: deviceName() }),
        });
        if (!res.ok) throw new Error(await res.text());
        const { token } = await res.json();
        if (live) saveToken(token);
      } catch (e) {
        if (!live) return;
        setError((e as Error).message || "pairing failed");
        setState("failed");
      }
    })();
    return () => {
      live = false;
    };
  }, [code]);

  return (
    <main className="pair">
      <div className="pair-mark" aria-hidden>
        ◆
      </div>
      <h1>Valkyrie</h1>
      {state === "pairing" ? (
        <p className="pair-lead">Pairing this {deviceName()}…</p>
      ) : (
        <>
          <p className="pair-lead">
            {status === "unpaired" && !error
              ? "This device isn't paired anymore."
              : "Pair this device to drive your agents from here."}
          </p>
          {error && <p className="pair-error">{error}</p>}
          <ol className="pair-steps">
            <li>
              On the machine running Valkyrie, run <code>valk web pair</code>
            </li>
            <li>Scan the QR code it shows with this device's camera</li>
          </ol>
          <p className="pair-note">The code works once, for ten minutes.</p>
        </>
      )}
    </main>
  );
}
