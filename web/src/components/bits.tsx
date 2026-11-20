import { STATE_LABEL, type AgentState } from "../proto";
import { useApp } from "../store";

/** A session's state as a dot (the word beside it says which). */
export function StateIcon({ state }: { state: AgentState }) {
  return <span className={`state-icon state-text-${state}`} role="img" aria-label={STATE_LABEL[state]} />;
}

const LABEL = {
  connecting: "connecting",
  open: "live",
  reconnecting: "reconnecting",
  offline: "offline",
  unpaired: "unpaired",
  outdated: "update valk",
  updated: "reload",
} as const;

export function ConnectionPill() {
  const status = useApp((s) => s.status);
  if (status === "updated") {
    return (
      <button className="pill conn-updated" title="Valkyrie was upgraded: reload for the new app" onClick={() => location.reload()}>
        <span className="pill-dot" />
        {LABEL[status]}
      </button>
    );
  }
  return (
    <span className={`pill conn-${status}`} title={status === "outdated" ? "This app and the daemon speak different protocols" : undefined}>
      <span className="pill-dot" />
      {LABEL[status]}
    </span>
  );
}

/** A page still loading: rows shaped like the list it will show. */
export function Loading({ rows = 4 }: { rows?: number }) {
  return (
    <div className="list skeleton" aria-busy="true" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="row">
          <span className="sk sk-chip" />
          <span className="row-main">
            <span className="sk sk-line" style={{ width: `${70 - ((i * 17) % 35)}%` }} />
            <span className="sk sk-line sk-sub" style={{ width: `${45 - ((i * 11) % 20)}%` }} />
          </span>
        </div>
      ))}
    </div>
  );
}
