import { useState } from "react";
import { lookList, lookText, setLook, useLook, useLooksVersion } from "../lib/theme";
import { proposalPopups, setProposalPopups } from "../lib/proposals";
import { AddDeviceSettings } from "./AddDevice";
import { Back, Check } from "./icons";
import { NotifySettings } from "./Notify";
import { Backdrop } from "./Backdrop";
import { usePageKeys } from "../lib/pageKeys";

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
