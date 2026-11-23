// The records in the attention queue (#251): decision proposals, the supervisor's
// ready items (Decide), and agent-filed backlog items batched
// per repository (Filed). Each is a row with a square marker that opens into the
// card that answers it; sessions keep their round dot and their own card.
import { useMemo, useState } from "react";
import { attention, filedLine, type Attn } from "../lib/attention";
import { base, launchesOn } from "../lib/backlog";
import { HASH } from "../lib/routes";
import { age } from "../lib/time";
import { answerLaunch, useApp } from "../store";
import type { LaunchOffer } from "../proto";
import { ProposalCard } from "./Decisions";
import { Chevron } from "./icons";

/** Everything waiting on you, in the order it's drawn: one list, one count. */
export function useAttention(): Attn[] {
  const queue = useApp((s) => s.queue);
  const proposals = useApp((s) => s.proposals);
  const launches = useApp((s) => s.launches);
  const items = useApp((s) => s.items);
  return useMemo(
    () => attention({ queue, proposals, launches, items }),
    [queue, proposals, launches, items],
  );
}

export type RecordItem = Exclude<Attn, { kind: "ask" | "stuck" | "done" }>;

export const isRecord = (a: Attn): a is RecordItem => a.kind === "decide" || a.kind === "filed";

const triage = () => {
  location.hash = HASH.backlog;
};

/** A record's row: its name, where it's from, what it asks, and how long it's waited. */
function line(a: RecordItem): { name: string; where: string; ask: string } {
  if (a.kind === "filed")
    return { name: filedLine(a), where: "", ask: a.items.map((i) => i.title).join(" · ") };
  switch (a.what) {
    case "decision":
      return {
        name: a.decision.title,
        where: `${base(a.decision.project)} · ${a.decision.kind} #${a.decision.id}`,
        ask: a.decision.fresh?.review ? "still true?" : "to accept or reject",
      };
    case "launch":
      return { name: a.offer.title, where: `${base(a.offer.project)}#${a.offer.number}`, ask: `ready to start · ${a.offer.why}` };
  }
}

/**
 * One record in Needs you. A Filed row opens the backlog to triage; a Decide row
 * opens into the card that answers it, while it's picked or tapped open.
 */
export function RecordRow({
  a,
  picked,
  open,
  current,
  onToggle,
}: {
  a: RecordItem;
  picked: boolean;
  open: boolean;
  /** Its card is in the pane beside the list, so it isn't drawn here too. */
  current: boolean;
  onToggle: () => void;
}) {
  const now = useApp((s) => s.now);
  const { name, where, ask } = line(a);
  const shown = (open || picked) && !current;
  return (
    <div className={`attn-slot ${picked ? "picked" : ""}`} data-pick>
      <button
        className={`row attn-row ${picked ? "picked" : ""} ${current ? "current" : ""}`}
        aria-current={current || undefined}
        aria-expanded={a.kind === "decide" ? shown : undefined}
        onClick={a.kind === "filed" ? triage : onToggle}
      >
        <span className={`attn-mark attn-${a.kind}`} role="img" aria-label={a.kind === "filed" ? "Filed" : "Decide"} />
        <span className="row-main">
          <span className="row-name">{name}</span>
          <span className="row-sub">
            {where && <span className="attn-where">{where} · </span>}
            {ask}
          </span>
        </span>
        <span className="row-age">{age(a.since, now)}</span>
        {a.kind === "filed" ? <span className="attn-go">Triage</span> : <Chevron />}
      </button>
      {a.kind === "decide" && shown && <RecordCard a={a} />}
    </div>
  );
}

/** The card that answers a record: Accept · Edit · Reject for a decision, Start or
 * Leave for a ready item, Triage for filed items. */
export function RecordCard({ a }: { a: RecordItem }) {
  if (a.kind === "filed") return <FiledCard a={a} />;
  switch (a.what) {
    case "decision":
      return <ProposalCard d={a.decision} />;
    case "launch":
      return <LaunchCard offer={a.offer} />;
  }
}

function LaunchCard({ offer }: { offer: LaunchOffer }) {
  const [busy, setBusy] = useState(false);
  const act = async (start: boolean) => {
    setBusy(true);
    try {
      await answerLaunch(offer, start);
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="supervisor-card">
      <div className="supervisor-card-head">
        <span className="tag">ready</span>
        <span className="supervisor-card-title">
          #{offer.number} {offer.title}
        </span>
      </div>
      <div className="supervisor-card-sub">
        {offer.why} · on {launchesOn(offer)} · {base(offer.project)}
      </div>
      <div className="order-actions">
        <button className="ghost" disabled={busy} onClick={() => void act(false)}>
          Leave
        </button>
        <button className="primary" disabled={busy} onClick={() => void act(true)}>
          Start
        </button>
      </div>
    </section>
  );
}

function FiledCard({ a }: { a: Extract<RecordItem, { kind: "filed" }> }) {
  return (
    <section className="supervisor-card">
      <div className="supervisor-card-head">
        <span className="tag">filed</span>
        <span className="supervisor-card-title">{filedLine(a)}</span>
      </div>
      <ul className="attn-filed-list">
        {a.items.map((i) => (
          <li key={i.id}>
            <span className="attn-where">#{i.number}</span> {i.title}
          </li>
        ))}
      </ul>
      <div className="order-actions">
        <button className="primary" onClick={triage}>
          Triage
        </button>
      </div>
    </section>
  );
}
