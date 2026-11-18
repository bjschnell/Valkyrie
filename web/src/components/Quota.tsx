import { read } from "../lib/quota";
import { useApp } from "../store";

/** How much of the Claude plan is used, a bar per window, so a long session isn't
 * started on the last few percent. Nothing until Claude Code has said. `quiet`
 * drops the bars to a line of text, for the inbox, where it isn't the point. */
export function QuotaMeter({ quiet = false }: { quiet?: boolean } = {}) {
  const quota = useApp((s) => s.quota);
  const now = useApp((s) => s.now);
  if (!quota || quota.limits.length === 0) return null;
  const { windows, stale } = read(quota, now);
  return (
    <section className={`quota${quiet ? " quiet-quota" : ""}${stale ? " stale" : ""}`} aria-label="Claude quota">
      <span className="quota-who">Claude</span>
      {windows.map((w) => (
        <div
          key={w.label}
          className={`quota-window quota-${w.level}`}
          title={w.resets ?? undefined}
        >
          <span className="quota-label">{w.label}</span>
          {!quiet && (
            <span className="quota-bar" aria-hidden>
              <span style={{ width: `${w.used}%` }} />
            </span>
          )}
          <span className="quota-used">
            {w.level === "reset" ? "reset" : `${w.used}%`}
          </span>
        </div>
      ))}
      {stale && <span className="quota-age">{stale}</span>}
    </section>
  );
}
