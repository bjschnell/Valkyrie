import { useEffect, useState } from "react";
import { lookList, lookText, setLook, useLook, useLooksVersion } from "../lib/theme";
import { proposalPopups, setProposalPopups } from "../lib/proposals";
import { AddDeviceSettings } from "./AddDevice";
import { Back, Check } from "./icons";
import { NotifySettings } from "./Notify";
import { Backdrop } from "./Backdrop";
import { usePageKeys } from "../lib/pageKeys";
import { useApp, webServerConfig } from "../store";
import type { WebServerState } from "../proto";

const home = () => {
  location.hash = "#/";
};

/** The look, notifications and adding a device: what used to be three icons in the inbox's bar. */
export function Settings() {
  const look = useLook();
  // A pack arriving adds to the list.
  useLooksVersion();

  // q or Esc goes back, when you aren't typing.
  usePageKeys("settings", { q: home, Escape: home });

  return (
    <div className="page">
      <Backdrop look={look} />
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">Settings</h1>
        </div>
      </header>

      <main className="inbox">
        <section>
          <h2 className="section-title">Look</h2>
          <div className="list" role="radiogroup" aria-label="Look">
            {lookList().map((l) => (
              <button key={l} className="row" role="radio" aria-checked={look === l} onClick={() => setLook(l)}>
                <span className="row-main">
                  <span className="row-name">{lookText(l).name}</span>
                  <span className="row-sub">{lookText(l).about}</span>
                </span>
                <span className="look-check">{look === l && <Check />}</span>
              </button>
            ))}
          </div>
        </section>
        <NotifySettings />
        <ProposalSettings />
        <WebServerSettings />
        <AddDeviceSettings />
      </main>
    </div>
  );
}


/** Whether an agent's new backlog proposal pops up here (#194): this browser's own. */
function ProposalSettings() {
  const [on, setOn] = useState(proposalPopups);
  const flip = () => {
    setProposalPopups(!on);
    setOn(!on);
  };
  return (
    <section>
      <h2 className="section-title">Agent proposals</h2>
      <div className="list">
        <button className="row" role="switch" aria-checked={on} onClick={flip}>
          <span className="row-main">
            <span className="row-name">Pop up new ones</span>
            <span className="row-sub">
              When an agent files a backlog item, a toast lets you accept or decline it, or tap through to the
              backlog. This browser only.
            </span>
          </span>
          <span className="look-check">{on && <Check />}</span>
        </button>
      </div>
    </section>
  );
}

/** What each state of the daemon's web server reads as: a name and a line. */
function runText(s: WebServerState): { name: string; sub: string } {
  switch (s.run) {
    case "running":
      return { name: "Running", sub: `On http://${s.listen}` };
    case "starting":
      return { name: "Starting", sub: `On http://${s.listen}, in a few seconds` };
    case "off":
      return { name: "Off", sub: "valk web or valk setup web still start it" };
    case "service":
      return { name: "Stood aside", sub: "The service from valk setup web runs it" };
    case "taken":
      return { name: "Port taken", sub: `Something else listens on ${s.listen}: pick another port` };
    case "unmanaged":
      return { name: "Not kept here", sub: "This daemon doesn't start a web server" };
  }
}

/** The host part of a listen address and its port: `[::1]:9000` → `::1`, `9000`. */
function splitListen(listen: string): { host: string; port: string } {
  const at = listen.lastIndexOf(":");
  return { host: listen.slice(0, at).replace(/^\[|\]$/g, ""), port: listen.slice(at + 1) };
}

/** `host` with `port`, bracketed when it's IPv6. */
function joinListen(host: string, port: string): string {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

const LOCAL = "127.0.0.1";
const EVERY = "0.0.0.0";
/** How often the page asks what the server is doing while it's open. */
const POLL_MS = 2000;

/** The daemon's web server (#316, #321): on or off, where it listens, the
 * address phones open, and what it's doing. The daemon's settings, shared with
 * valk's settings panel. Turning it off, or opening it to every interface,
 * takes a second tap. */
function WebServerSettings() {
  const [state, setState] = useState<WebServerState | null>(null);
  const [arming, setArming] = useState<"off" | "every" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [port, setPort] = useState("");
  const [url, setUrl] = useState("");
  // What the fields were filled from, so a poll doesn't overwrite your typing.
  const [shown, setShown] = useState<WebServerState | null>(null);
  // Asked once the connection is up: a page opened straight on Settings has none yet.
  const open = useApp((s) => s.status === "open");
  const apply = (next: Promise<WebServerState>) =>
    next
      .then((now) => {
        setState(now);
        setError(null);
      })
      .catch((e: unknown) => setError(String(e instanceof Error ? e.message : e)));
  useEffect(() => {
    if (!open) return;
    void apply(webServerConfig());
    const timer = setInterval(() => void apply(webServerConfig()), POLL_MS);
    return () => clearInterval(timer);
  }, [open]);
  useEffect(() => {
    if (!state) return;
    if (shown && shown.listen === state.listen && shown.url === state.url) return;
    setPort(splitListen(state.listen).port);
    setUrl(state.url ?? "");
    setShown(state);
  }, [state, shown]);
  const change = (c: { on?: boolean; listen?: string; url?: string }) => {
    setArming(null);
    void apply(webServerConfig(c));
  };

  const on = state?.on ?? false;
  const host = state ? splitListen(state.listen).host : LOCAL;
  const custom = state !== null && host !== LOCAL && host !== EVERY;
  const tapOn = () => {
    if (!state) return;
    if (!on) return change({ on: true });
    if (arming !== "off") return setArming("off");
    change({ on: false });
  };
  const listenOn = (next: string) => {
    if (!state || next === host) return;
    if (next === EVERY && arming !== "every") return setArming("every");
    change({ listen: joinListen(next, splitListen(state.listen).port) });
  };
  const savePort = () => {
    if (!state || port.trim() === splitListen(state.listen).port) return;
    change({ listen: joinListen(host, port.trim()) });
  };
  const saveUrl = () => {
    if (!state || url.trim() === (state.url ?? "")) return;
    change({ url: url.trim() });
  };
  const enter = (save: () => void) => (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      save();
    }
  };
  const run = state && runText(state);

  // A row's sub is one line: a prompt goes in its name.
  return (
    <section>
      <h2 className="section-title">Web server</h2>
      <div className="list">
        <button
          className="row"
          role="switch"
          aria-checked={on}
          disabled={!state}
          onClick={tapOn}
          onBlur={() => setArming(null)}
        >
          <span className="row-main">
            <span className="row-name">{arming === "off" ? "Tap again to turn it off" : "Start with Valkyrie"}</span>
            <span className="row-sub">
              {arming === "off" ? "This page stops too if the daemon serves it" : "Keeps this app running on this machine"}
            </span>
          </span>
          <span className="look-check">{on && <Check />}</span>
        </button>
        {run && (
          <div className="row web-run" data-run={state.run} role="status">
            <span className="row-main">
              <span className="row-name">{run.name}</span>
              <span className="row-sub">{error ?? run.sub}</span>
            </span>
          </div>
        )}
      </div>
      <h2 className="section-title">Listen on</h2>
      <div className="list" role="radiogroup" aria-label="Listen on">
        <button className="row" role="radio" aria-checked={host === LOCAL} disabled={!state} onClick={() => listenOn(LOCAL)}>
          <span className="row-main">
            <span className="row-name">This machine</span>
            <span className="row-sub">Recommended. Tailscale serve or a proxy here forwards to it</span>
          </span>
          <span className="look-check">{host === LOCAL && <Check />}</span>
        </button>
        <button
          className="row"
          role="radio"
          aria-checked={host === EVERY}
          disabled={!state}
          onClick={() => listenOn(EVERY)}
          onBlur={() => setArming(null)}
        >
          <span className="row-main">
            <span className="row-name">{arming === "every" ? "Tap again to listen on every interface" : "All interfaces"}</span>
            <span className="row-sub">
              {arming === "every"
                ? "Anyone who can reach the port can try to pair"
                : "For Docker or a proxy on another host, at your own risk"}
            </span>
          </span>
          <span className="look-check">{host === EVERY && <Check />}</span>
        </button>
        {custom && (
          <div className="row" role="radio" aria-checked="true" aria-disabled="true">
            <span className="row-main">
              <span className="row-name">{host}</span>
              <span className="row-sub">Set in settings.toml; pick one above to change it</span>
            </span>
            <span className="look-check">
              <Check />
            </span>
          </div>
        )}
      </div>
      <div className="web-fields">
        <label className="field">
          <span className="field-label">Port</span>
          <input
            inputMode="numeric"
            value={port}
            disabled={!state}
            onChange={(e) => setPort(e.target.value.replace(/\D/g, "").slice(0, 5))}
            onBlur={savePort}
            onKeyDown={enter(savePort)}
          />
        </label>
        <label className="field">
          <span className="field-label">Phone URL</span>
          <input
            type="url"
            inputMode="url"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            placeholder={state?.default_url ?? ""}
            value={url}
            disabled={!state}
            onChange={(e) => setUrl(e.target.value)}
            onBlur={saveUrl}
            onKeyDown={enter(saveUrl)}
          />
        </label>
      </div>
    </section>
  );
}
