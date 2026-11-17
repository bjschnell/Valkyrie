import { describe, expect, it } from "vitest";
import { newProposals, proposalToast } from "./proposals";
import type { Item } from "../proto";

const item = (id: number, status: string, by: string, extra: Partial<Item> = {}): Item =>
  ({ id, number: id, title: `item ${id}`, status, by, created: 0, updated: 0, ...extra }) as unknown as Item;

describe("newProposals", () => {
  it("is news only for a proposal that arrives after connecting, once", () => {
    const first = [item(1, "proposed", "claude")];
    let { known, fresh } = newProposals(null, first);
    expect(fresh).toEqual([]);

    const next = [...first, item(2, "proposed", "codex"), item(3, "open", "human")];
    ({ known, fresh } = newProposals(known, next));
    expect(fresh.map((i) => i.id)).toEqual([2]);
    ({ known, fresh } = newProposals(known, next));
    expect(fresh).toEqual([]);
  });

  it("skips one reopened back to proposed, and an agent's own taken work", () => {
    const { known } = newProposals(null, [item(1, "open", "claude")]);
    const own = item(2, "proposed", "claude", { filed_by: [7, 3], taken: [3] });
    expect(newProposals(known, [item(1, "proposed", "claude"), own]).fresh).toEqual([]);
  });
});

describe("proposalToast", () => {
  it("names one by its agent, several by count, and answers the newest", () => {
    expect(proposalToast([])).toBeNull();
    expect(proposalToast([item(5, "proposed", "claude")])).toEqual({ text: "claude proposed #5 item 5", id: 5 });
    expect(proposalToast([item(9, "proposed", "codex"), item(5, "proposed", "claude")])).toEqual({
      text: "2 agent proposals, newest #9 item 9",
      id: 9,
    });
  });
});
