import { useEffect, useState } from "react";
import { lookList, lookText, setLook, useLook, useLooksVersion } from "../lib/theme";
import { proposalPopups, setProposalPopups } from "../lib/proposals";
import { AddDeviceSettings } from "./AddDevice";
import { Back, Check } from "./icons";
import { NotifySettings } from "./Notify";
import { Backdrop } from "./Backdrop";
import { usePageKeys } from "../lib/pageKeys";
import { useApp, webServer } from "../store";

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

/** Whether the daemon keeps this web app running on localhost (#316): the
 * daemon's setting, shared with valk's settings panel. Turning it off can stop
 * the server this page came from, so that takes a second tap. */
function WebServerSettings() {
  const [on, setOn] = useState<boolean | null>(null);
  const [arming, setArming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Asked once the connection is up: a page opened straight on Settings has none yet.
  const open = useApp((s) => s.status === "open");
  useEffect(() => {
    if (!open) return;
    webServer()
      .then((now) => {
        setOn(now);
        setError(null);
      })
      .catch((e: unknown) => setError(String(e instanceof Error ? e.message : e)));
  }, [open]);
  const set = (next: boolean) => {
    setArming(false);
    webServer(next)
      .then((now) => {
        setOn(now);
        setError(null);
      })
      .catch((e: unknown) => setError(String(e instanceof Error ? e.message : e)));
  };
  const tap = () => {
    if (on === null) return;
    if (!on) return set(true);
    if (!arming) return setArming(true);
    set(false);
  };
  // A row's sub is one line: the prompt goes in its name.
  const name = arming ? "Tap again to turn it off" : "Start with Valkyrie";
  const sub = arming
    ? "This page stops too if the daemon serves it"
    : "Keeps this app running here, on localhost only";
  return (
    <section>
      <h2 className="section-title">Web server</h2>
      <div className="list">
        <button className="row" role="switch" aria-checked={on === true} disabled={on === null} onClick={tap} onBlur={() => setArming(false)}>
          <span className="row-main">
            <span className="row-name">{name}</span>
            <span className="row-sub">{error ?? sub}</span>
          </span>
          <span className="look-check">{on && <Check />}</span>
        </button>
      </div>
    </section>
  );
}
