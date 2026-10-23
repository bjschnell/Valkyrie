import type { AgentState } from "../proto";
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
    <span className={`pill conn-${status}`} title={status === "outdated" ? "This app and the daemon speak different protocols: restart valk web (valk setup web does it)" : undefined}>
      <span className="pill-dot" />
      {LABEL[status]}
    </span>
  );
}
