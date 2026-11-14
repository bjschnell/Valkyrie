import type { AgentState } from "../proto";
import { useSpike } from "../lib/trial";
import { useApp } from "../store";

/** The TUI's state glyphs; a spinner while working. */
export function StateIcon({ state }: { state: AgentState }) {
  if (state === "working") return <span className="spin" aria-label="working" />;
  const glyph: Record<AgentState, string> = {
    working: "",
    needs_input: "●",
    blocked: "×",
    review_ready: "✓",
    interrupted: "◆",
    stale: "◇",
    idle: "○",
    exited: "·",
  };
  return (
    <span className={`state-icon state-text-${state}`} aria-label={state}>
      {glyph[state]}
    </span>
  );
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

/** A page still loading: rows shaped like the list it will show (#104 trial),
 * or the plain word with the trial off. */
export function Loading({ rows = 4 }: { rows?: number }) {
  const shaped = useSpike("cache");
  if (!shaped) return <div className="empty">Loading…</div>;
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
