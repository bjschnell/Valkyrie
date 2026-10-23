import { useEffect } from "react";
import { LOOKS, LOOK_TEXT, setLook, useLook } from "../lib/theme";
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
      {look !== "default" && <Rain key={look} />}
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Settings</div>
        </div>
      </header>

      <main className="inbox">
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
