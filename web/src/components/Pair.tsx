import { useEffect, useState } from "react";
import { DEV_TOKEN, parseCode } from "../lib/pair";
import { saveToken, useApp } from "../store";
import { Wordmark } from "./Logo";

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
  const dev = typed.trim() === DEV_TOKEN;

  return (
    <main className="pair">
      <img className="pair-art" src="/icons/icon-512.png" alt="" />
      <h1>
        <Wordmark />
      </h1>
      <p className="pair-tagline">AI agent command center</p>
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
              On a device that's already paired, tap <b>Add a device</b> and type the code it shows below. Or, on
              the machine running Valkyrie, press <code>p</code> in <code>valk</code>, or run{" "}
              <code>valk web pair</code>.
            </li>
            {desktop ? (
              <li>
                On that machine itself, <code>valk web open</code> opens Valkyrie here, already paired.
              </li>
            ) : (
              <li>
                Or scan the QR code <code>valk web pair</code> shows. On iPhone that opens Safari, not the installed
                app, so type the code here instead.
              </li>
            )}
          </ol>
          <form
            className="pair-paste"
            onSubmit={(e) => {
              e.preventDefault();
              if (dev) {
                saveToken(DEV_TOKEN);
              } else if (pasted) {
                setError("");
                setCode(pasted);
                setTyped("");
              }
            }}
          >
            <input
              aria-label="Pairing code or link"
              placeholder="Code (KQ7-4M2) or pairing link"
              autoCapitalize="characters"
              autoComplete="off"
              spellCheck={false}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
            />
            <button className="primary" type="submit" disabled={!pasted && !dev}>
              Pair
            </button>
          </form>
          <p className="pair-note">
            A code works once, for a few minutes. Once paired, this device stays paired until you revoke it.
          </p>
        </>
      )}
    </main>
  );
}
