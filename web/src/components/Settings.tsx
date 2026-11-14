import { SPIKES, SPIKE_TEXT, setSpike, useSpike, type Spike } from "../lib/trial";
import { useEffect } from "react";
import { LOOKS, LOOK_TEXT, hasRain, setLook, useLook } from "../lib/theme";
import { AddDeviceSettings } from "./AddDevice";
import { Back, Check } from "./icons";
import { NotifySettings } from "./Notify";
import { Rain } from "./Rain";

const home = () => {
  location.hash = "#/";
};

/** The look, notifications and adding a device: what used to be three icons in the inbox's bar. */
export function Settings() {
  const look = useLook();

  // q or Esc goes back, when you aren't typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        home();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="page">
      {hasRain(look) && <Rain key={look} />}
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Settings</div>
        </div>
      </header>

      <main className="inbox">
        <TrialSettings />
        <section>
          <h2 className="section-title">Look</h2>
          <div className="list" role="radiogroup" aria-label="Look">
            {LOOKS.map((l) => (
              <button key={l} className="row" role="radio" aria-checked={look === l} onClick={() => setLook(l)}>
                <span className="row-main">
                  <span className="row-name">{LOOK_TEXT[l].name}</span>
                  <span className="row-sub">{LOOK_TEXT[l].about}</span>
                </span>
                <span className="look-check">{look === l && <Check />}</span>
              </button>
            ))}
          </div>
        </section>
        <NotifySettings />
        <AddDeviceSettings />
      </main>
    </div>
  );
}

/** #104 trial: switch each UI spike off to compare it with the app as it was. */
function TrialSettings() {
  const nav = useSpike("nav");
  const cache = useSpike("cache");
  const motion = useSpike("motion");
  const on: Record<Spike, boolean> = { nav, cache, motion };
  return (
    <section>
      <h2 className="section-title">Trial: new UI</h2>
      <div className="list">
        {SPIKES.map((s) => (
          <button key={s} className="row" role="switch" aria-checked={on[s]} onClick={() => setSpike(s, !on[s])}>
            <span className="row-main">
              <span className="row-name">{SPIKE_TEXT[s].name}</span>
              <span className="row-sub">{SPIKE_TEXT[s].about}</span>
            </span>
            <span className={`trial-switch${on[s] ? " on" : ""}`} />
          </button>
        ))}
      </div>
    </section>
  );
}
