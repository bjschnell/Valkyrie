// The host switcher (#179): in the desktop app's rail, which machine this window
// shows, and a box to pick another or connect to a new one over SSH. The app
// opens each host in a window of its own; see lib/hosts.ts.

import { useEffect, useRef, useState, type FormEvent } from "react";
import { hostSignal, readHosts, validHost, type Hosts } from "../lib/hosts";
import { Server } from "./icons";

/** The hosts as the app last said, kept current. */
function useHosts(): Hosts | null {
  const [hosts, setHosts] = useState(readHosts);
  useEffect(() => {
    const changed = () => setHosts(readHosts());
    window.addEventListener("valk-hosts", changed);
    return () => window.removeEventListener("valk-hosts", changed);
  }, []);
  return hosts;
}

/** The rail's button: this window's host. Nothing outside the desktop app. */
export function HostButton() {
  const hosts = useHosts();
  const [open, setOpen] = useState(false);
  if (!hosts) return null;
  const here = hosts.current ?? "This computer";
  return (
    <>
      <button
        className="rail-item rail-host"
        aria-label={`Host: ${here}. Switch host`}
        title={`${here}: switch host`}
        onClick={() => setOpen(true)}
      >
        <span className="nav-icon">
          <Server />
        </span>
        <span className="nav-label">{hosts.current ?? "Here"}</span>
      </button>
      {open && <HostsDialog hosts={hosts} onClose={() => setOpen(false)} />}
    </>
  );
}

function HostsDialog({ hosts, onClose }: { hosts: Hosts; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const back = useRef<HTMLElement | null | undefined>(undefined);
  if (back.current === undefined) back.current = document.activeElement as HTMLElement | null;
  useEffect(() => {
    if (!dialog.current?.open) dialog.current?.showModal();
    return () => back.current?.focus?.({ preventScroll: true });
  }, []);
  const downOn = useRef<EventTarget | null>(null);
  const [name, setName] = useState("");
  const typed = name.trim();
  const ok = validHost(typed);

  const go = (host: string | null) => {
    onClose();
    // The app opens the window; this page stays as it is.
    if (host !== hosts.current) location.href = hostSignal(host);
  };
  const connect = (e: FormEvent) => {
    e.preventDefault();
    if (ok) go(typed);
  };

  return (
    <dialog
      ref={dialog}
      className="keys hosts"
      aria-labelledby="hosts-title"
      onClose={onClose}
      onPointerDown={(e) => (downOn.current = e.target)}
      onClick={(e) => e.target === dialog.current && downOn.current === dialog.current && onClose()}
    >
      <div className="keys-box">
        <h2 className="keys-title" id="hosts-title">
          Hosts
        </h2>
        <div className="keys-body">
          <div className="list">
            {[null, ...hosts.hosts].map((h) => (
              <button
                key={h ?? ""}
                className={`row${h === hosts.current ? " current" : ""}`}
                aria-current={h === hosts.current ? "page" : undefined}
                onClick={() => go(h)}
              >
                <span className="row-main">
                  <span className="row-name">{h ?? "This computer"}</span>
                </span>
              </button>
            ))}
          </div>
          <form className="hosts-add" onSubmit={connect}>
            <label className="field">
              <span className="field-label">Connect to a host</span>
              <div className="field-row">
                <input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="box, me@box, or an alias from ~/.ssh/config"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  aria-invalid={typed !== "" && !ok}
                />
                <button className="primary" type="submit" disabled={!ok}>
                  Connect
                </button>
              </div>
            </label>
            <p className="hosts-note">
              Over SSH, with your key or agent: Valkyrie starts the host's web app if it isn't running and opens it in a
              window of its own.
            </p>
          </form>
        </div>
      </div>
    </dialog>
  );
}
