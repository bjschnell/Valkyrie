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
} as const;

export function ConnectionPill() {
  const status = useApp((s) => s.status);
  return (
    <span className={`pill conn-${status}`} title={status === "outdated" ? "This app and the daemon speak different protocols: run install.sh, then valk upgrade" : undefined}>
      <span className="pill-dot" />
      {LABEL[status]}
    </span>
  );
}
