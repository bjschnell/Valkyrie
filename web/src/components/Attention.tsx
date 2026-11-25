// The records in the attention queue (#251): a disk running low (#300), decision
// proposals, the supervisor's ready items (Decide), and agent-filed backlog items
// batched per repository (Filed). Each is a row with a square marker that opens into the
// card that answers it; sessions keep their round dot and their own card.
import { useMemo, useState } from "react";
import { attention, bytes, diskLine, filedLine, type Attn } from "../lib/attention";
import { base, launchesOn } from "../lib/backlog";
import { HASH } from "../lib/routes";
import { age } from "../lib/time";
import { answerLaunch, freeDisk, useApp } from "../store";
import type { LaunchOffer } from "../proto";
import { ProposalCard } from "./Decisions";
import { Chevron } from "./icons";

/** Everything waiting on you, in the order it's drawn: one list, one count. */
export function useAttention(): Attn[] {
  const queue = useApp((s) => s.queue);
  const proposals = useApp((s) => s.proposals);
  const launches = useApp((s) => s.launches);
  const items = useApp((s) => s.items);
  const disk = useApp((s) => s.disk);
  return useMemo(
    () => attention({ queue, proposals, launches, items, disk }),
    [queue, proposals, launches, items, disk],
  );
}

export type RecordItem = Exclude<Attn, { kind: "ask" | "stuck" | "done" }>;

export const isRecord = (a: Attn): a is RecordItem =>
  a.kind === "decide" || a.kind === "filed" || a.kind === "disk";

const MARK: Record<RecordItem["kind"], string> = { disk: "Disk low", decide: "Decide", filed: "Filed" };

const triage = () => {
  location.hash = HASH.backlog;
};

/** A record's row: its name, where it's from, what it asks, and how long it's waited. */
function line(a: RecordItem): { name: string; where: string; ask: string } {
  if (a.kind === "filed")
    return { name: filedLine(a), where: "", ask: a.items.map((i) => i.title).join(" · ") };
  if (a.kind === "disk") return { name: "Disk low", where: "", ask: diskLine(a.low) };
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
        aria-expanded={a.kind !== "filed" ? shown : undefined}
        onClick={a.kind === "filed" ? triage : onToggle}
      >
        <span className={`attn-mark attn-${a.kind}`} role="img" aria-label={MARK[a.kind]} />
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
      {a.kind !== "filed" && shown && <RecordCard a={a} />}
    </div>
  );
}

/** The card that answers a record: Accept · Edit · Reject for a decision, Start or
 * Leave for a ready item, Triage for filed items. */
export function RecordCard({ a }: { a: RecordItem }) {
  if (a.kind === "filed") return <FiledCard a={a} />;
  if (a.kind === "disk") return <DiskCard a={a} />;
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

/** A disk running low: how much is free on each, and Free space, which deletes
 * every worktree's cargo `target/` nothing built in for an hour. */
function DiskCard({ a }: { a: Extract<RecordItem, { kind: "disk" }> }) {
  const [busy, setBusy] = useState(false);
  const free = async () => {
    setBusy(true);
    try {
      await freeDisk();
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="supervisor-card">
      <div className="supervisor-card-head">
        <span className="tag">disk</span>
        <span className="supervisor-card-title">Disk low</span>
      </div>
      <ul className="attn-filed-list">
        {a.low.map((d) => (
          <li key={d.path}>
            {bytes(d.free)} free of {bytes(d.total)} <span className="attn-where">{d.path}</span>
          </li>
        ))}
      </ul>
      <div className="supervisor-card-sub">
        Agents' worktrees build their own target/, up to 24 GB each. Free space deletes each one nothing built in for an hour.
      </div>
      <div className="order-actions">
        <button className="primary" disabled={busy} onClick={() => void free()}>
          {busy ? "Freeing…" : "Free space"}
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
