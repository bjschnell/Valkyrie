// The supervisor page (backlog #180): what waits on you, gathered from what the
// app already holds, and the home strip's one line.
import { age } from "./time";
import type { Decision, LaunchOffer, OrderProposal, QueueItem, SupervisorEntry } from "../proto";

/** Something the supervisor waits on you for. */
export type Wait =
  | { kind: "flag"; item: QueueItem }
  | { kind: "launch"; offer: LaunchOffer }
  | { kind: "order"; order: OrderProposal }
  | { kind: "policy"; decision: Decision };

/** Everything waiting on you, flags first (a red or conflicting PR is the most
 * urgent), then ready items, build orders, and step policies to review. */
export function waitingOnYou(
  queue: QueueItem[],
  launches: LaunchOffer[],
  orders: OrderProposal[],
  proposals: Decision[],
): Wait[] {
  return [
    ...queue.filter((q) => q.flag?.step).map((item): Wait => ({ kind: "flag", item })),
    ...launches.map((offer): Wait => ({ kind: "launch", offer })),
    ...orders.map((order): Wait => ({ kind: "order", order })),
    ...proposals
      .filter((d) => d.kind === "policy" && d.status === "proposed")
      .map((decision): Wait => ({ kind: "policy", decision })),
  ];
}

/** The home strip's line: `3 waiting · policy #12: fix CI on PR #101`, or null
 * when there's nothing to say. */
export function stripLine(waiting: number, latest: SupervisorEntry | undefined): string | null {
  const parts = [];
  if (waiting) parts.push(`${waiting} waiting on you`);
  if (latest) parts.push(latest.said);
  return parts.length ? parts.join(" · ") : null;
}

/** The Watching section's line: `2 PRs · read 2m ago · next in 3m`. `nowMs` in
 * ms, the poll's times in seconds. */
export function watchLine(w: { prs: unknown[]; polled_at?: number; next_at?: number; on: boolean }, nowMs: number): string {
  if (!w.on) return "off: nothing is read";
  if (w.polled_at === undefined) return "the poll hasn't run yet";
  const n = `${w.prs.length} PR${w.prs.length === 1 ? "" : "s"}`;
  const read = `read ${age(w.polled_at * 1000, nowMs)} ago`;
  const next = w.next_at !== undefined && w.next_at * 1000 > nowMs ? ` · next in ${age(nowMs, w.next_at * 1000)}` : "";
  return `${n} · ${read}${next}`;
}
