import { useEffect, useState } from "react";
import { parseCode } from "../lib/pair";
import { saveToken, useApp } from "../store";

/** A name for this device in `valk web devices`: the platform, roughly. */
function deviceName(): string {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android";
  if (/Mac/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows";
  if (/Linux/.test(ua)) return "Linux";
  return "browser";
}

/** A mouse, not a camera: say how to pair from this computer instead. */
function onDesktop(): boolean {
  return matchMedia("(pointer: fine)").matches;
}

export function Pair() {
  const status = useApp((s) => s.status);
  const [code, setCode] = useState(() => (/^#pair=/.test(location.hash) ? parseCode(location.hash) : null));
  const [state, setState] = useState<"idle" | "pairing" | "failed">(code ? "pairing" : "idle");
  const [error, setError] = useState("");
  const [typed, setTyped] = useState("");
  const [desktop] = useState(onDesktop);

  useEffect(() => {
    if (!code) return;
    // The code is single-use: out of the address bar (and history) at once.
    if (location.hash) history.replaceState(null, "", location.pathname);
    let live = true;
    setState("pairing");
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
        setCode(null);
      }
    })();
    return () => {
      live = false;
    };
  }, [code]);

  const pasted = parseCode(typed);

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
          {desktop ? (
            <ol className="pair-steps">
              <li>
                On this computer, run <code>valk web open</code>. It opens Valkyrie here, paired.
              </li>
              <li>
                On another one, run <code>valk web pair</code> on the machine running Valkyrie and paste the link it
                prints below
              </li>
            </ol>
          ) : (
            <ol className="pair-steps">
              <li>
                On the machine running Valkyrie, run <code>valk web pair</code>
              </li>
              <li>Scan the QR code it shows with this device's camera</li>
            </ol>
          )}
          <form
            className="pair-paste"
            onSubmit={(e) => {
              e.preventDefault();
              if (pasted) {
                setError("");
                setCode(pasted);
                setTyped("");
              }
            }}
          >
            <input
              aria-label="Pairing link or code"
              placeholder="Paste a pairing link or code"
              autoComplete="off"
              spellCheck={false}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
            />
            <button className="primary" type="submit" disabled={!pasted}>
              Pair
            </button>
          </form>
          <p className="pair-note">The code works once, for ten minutes.</p>
        </>
      )}
    </main>
  );
}
