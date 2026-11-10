import { read } from "../lib/quota";
import { useApp } from "../store";

/** How much of the Claude plan is used, a bar per window, so a long session isn't
 * started on the last few percent. Nothing until Claude Code has said. */
export function QuotaMeter() {
  const quota = useApp((s) => s.quota);
  const now = useApp((s) => s.now);
  if (!quota || quota.limits.length === 0) return null;
  const { windows, stale } = read(quota, now);
  return (
    <section className={`quota${stale ? " stale" : ""}`} aria-label="Claude quota">
      <span className="quota-who">Claude</span>
      {windows.map((w) => (
        <div
          key={w.label}
          className={`quota-window quota-${w.level}`}
          title={w.resets ?? undefined}
        >
          <span className="quota-label">{w.label}</span>
          <span className="quota-bar" aria-hidden>
            <span style={{ width: `${w.used}%` }} />
          </span>
          <span className="quota-used">
            {w.level === "reset" ? "reset" : `${w.used}%`}
          </span>
        </div>
      ))}
      {stale && <span className="quota-age">{stale}</span>}
    </section>
  );
}
