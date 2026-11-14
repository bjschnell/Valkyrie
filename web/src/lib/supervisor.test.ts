import { describe, expect, it } from "vitest";
import { stripLine, waitingOnYou } from "./supervisor";
import type { Decision, LaunchOffer, OrderProposal, QueueItem } from "../proto";

const queued = (session: number, step: boolean): QueueItem =>
  ({
    session,
    name: `s${session}`,
    cwd: "/r",
    status: { agent: "claude", state: "blocked", since_ms: 0 },
    flag: { reason: "PR #9 conflicts with main", pr: "u", seq: 1, step: step ? { action: "merge-base", why: "" } : undefined },
  }) as unknown as QueueItem;

const decision = (id: number, kind: string, status: string): Decision =>
  ({ id, project: "/r", title: "t", body: "", kind, status, created: 0, updated: 0 }) as unknown as Decision;

describe("waitingOnYou", () => {
  it("lists offered flags, ready items, orders and proposed policies, in that order", () => {
    const launch = { id: 1, project: "/r", item: 4 } as unknown as LaunchOffer;
    const order = { id: 2, project: "/r", steps: [] } as unknown as OrderProposal;
    const waits = waitingOnYou(
      [queued(1, true), queued(2, false)],
      [launch],
      [order],
      [decision(7, "policy", "proposed"), decision(8, "policy", "active"), decision(9, "gotcha", "proposed")],
    );
    expect(waits.map((w) => w.kind)).toEqual(["flag", "launch", "order", "policy"]);
    expect(waits[0].kind === "flag" && waits[0].item.session).toBe(1);
    expect(waits[3].kind === "policy" && waits[3].decision.id).toBe(7);
  });
});

describe("stripLine", () => {
  it("says what waits and the latest thing done, or nothing", () => {
    const latest = { at: 0, project: "/r", said: "policy #12: fix CI on PR #101", policy: 12 };
    expect(stripLine(3, latest)).toBe("3 waiting on you · policy #12: fix CI on PR #101");
    expect(stripLine(0, latest)).toBe("policy #12: fix CI on PR #101");
    expect(stripLine(0, undefined)).toBeNull();
  });
});
