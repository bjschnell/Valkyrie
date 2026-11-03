import { advice } from "../lib/protocol";
import { useApp } from "../store";
import { Wordmark } from "./Logo";

/** The daemon and this app can't talk: say so, with both sides, instead of an empty inbox. */
export function Outdated() {
  const mismatch = useApp((s) => s.mismatch);
  return (
    <main className="pair">
      <img className="pair-art" src="/icons/icon-512.png" alt="" />
      <h1>
        <Wordmark />
      </h1>
      <p className="pair-lead">This app and the daemon speak different protocols.</p>
      {mismatch && (
        <p className="pair-error">
          daemon speaks {mismatch.daemon}, this app speaks {mismatch.app ?? "an older one"}
        </p>
      )}
      <p className="pair-lead">
        {mismatch
          ? advice(mismatch)
          : "Run valk upgrade --local on the machine running Valkyrie, then restart valk web."}
      </p>
      <button className="primary" onClick={() => location.reload()}>
        Try again
      </button>
    </main>
  );
}
