import { useState } from "react";
import { DECISION_KINDS, type Decision, type DecisionKind, type ReviewAction } from "../proto";
import { age } from "../lib/time";
import { review, useApp } from "../store";

/** The last part of a path: the project's name. */
const base = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/**
 * Decisions agents proposed (ADR-0007): nothing reaches another agent until it's
 * accepted here, in the TUI, or with `valk decisions accept`.
 */
export function Proposals() {
  const proposals = useApp((s) => s.proposals);
  if (proposals.length === 0) return null;
  return (
    <section>
      <h2 className="section-title">
        Decisions to review <span className="count">{proposals.length}</span>
      </h2>
      <div className="cards">
        {proposals.map((d) => (
          <ProposalCard key={`${d.project}#${d.id}`} d={d} />
        ))}
      </div>
    </section>
  );
}

function ProposalCard({ d }: { d: Decision }) {
  const now = useApp((s) => s.now);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const act = async (f: () => Promise<boolean>) => {
    setBusy(true);
    await f();
    setBusy(false);
  };
  const who = { human: "you", valkyrie: "Valkyrie, from your correction" }[d.provenance.by] ?? d.provenance.by;
  const where = [base(d.project), d.provenance.session && d.provenance.session !== base(d.project) ? d.provenance.session : null]
    .filter(Boolean)
    .join(" · ");

  if (d.fresh?.review) return <StaleCard d={d} />;
  return (
    <div className="card-slot">
      <article className="card decision-card">
        <div className="card-head">
          <span className={`kind-chip kind-${d.kind}`}>{d.kind}</span>
          <span className="card-name">#{d.id}</span>
          <span className="card-prog">{where}</span>
          <span className="card-age">{age(d.created * 1000, now)}</span>
        </div>
        {editing ? (
          <DecisionEditor d={d} busy={busy} onDone={() => setEditing(false)} />
        ) : (
          <>
            <p className="decision-title">{d.title}</p>
            {d.body && <p className="decision-body">{d.body}</p>}
            <div className="decision-by">
              proposed by {who}
              {d.supersedes !== undefined && <> · replaces #{d.supersedes}</>}
            </div>
            <div className="card-actions">
              <button className="ghost no" disabled={busy} onClick={() => void act(() => review(d, { a: "reject" }))}>
                Reject
              </button>
              <button className="ghost" disabled={busy} onClick={() => setEditing(true)}>
                Edit
              </button>
              <button className="primary" disabled={busy} onClick={() => void act(() => review(d, { a: "accept" }))}>
                Accept
              </button>
            </div>
          </>
        )}
      </article>
    </div>
  );
}

/** An active decision that may be out of date: its files changed a lot, or it's
 * about something that changes. Agents still get it until it's retired. */
function StaleCard({ d }: { d: Decision }) {
  const [busy, setBusy] = useState(false);
  const act = async (action: ReviewAction) => {
    setBusy(true);
    await review(d, action);
    setBusy(false);
  };
  return (
    <div className="card-slot">
      <article className="card decision-card stale">
        <div className="card-head">
          <span className="kind-chip kind-gotcha">still true?</span>
          <span className="card-name">#{d.id}</span>
          <span className="card-prog">{base(d.project)}</span>
        </div>
        <p className="decision-title">{d.title}</p>
        {d.body && <p className="decision-body">{d.body}</p>}
        <div className="decision-by">{d.fresh?.review}</div>
        <div className="card-actions">
          <button className="ghost no" disabled={busy} onClick={() => void act({ a: "retire" })}>
            Retire
          </button>
          <button className="primary" disabled={busy} onClick={() => void act({ a: "confirm" })}>
            Still holds
          </button>
        </div>
      </article>
    </div>
  );
}

/** Rewords a proposal, then accepts it: the usual reason to edit one. */
function DecisionEditor({ d, busy, onDone }: { d: Decision; busy: boolean; onDone: () => void }) {
  const [title, setTitle] = useState(d.title);
  const [body, setBody] = useState(d.body);
  const [kind, setKind] = useState<DecisionKind>(d.kind);
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    const ok = await review(d, { a: "revise", title, body, kind });
    setSaving(false);
    if (ok) onDone();
  };
  return (
    <div className="decision-edit">
      <div className="kind-pick" role="radiogroup" aria-label="Kind">
        {DECISION_KINDS.map((k) => (
          <button
            key={k}
            role="radio"
            aria-checked={kind === k}
            className={`kind-chip kind-${k} ${kind === k ? "on" : ""}`}
            onClick={() => setKind(k)}
          >
            {k}
          </button>
        ))}
      </div>
      <label className="field">
        <span className="field-label">Title</span>
        <input value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
      </label>
      <label className="field">
        <span className="field-label">Why</span>
        <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={4} maxLength={4000} />
      </label>
      <div className="card-actions">
        <button className="ghost" disabled={saving} onClick={onDone}>
          Cancel
        </button>
        <button className="primary" disabled={busy || saving || !title.trim()} onClick={() => void save()}>
          Save &amp; accept
        </button>
      </div>
    </div>
  );
}
