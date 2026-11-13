import { describe, expect, it } from "vitest";
import { label, read } from "./quota";

const quota = (five: number, resets: number) => ({
  at: 1000,
  limits: [
    { name: "five_hour", used: five, resets_at: resets },
    { name: "seven_day_opus", used: 10 },
  ],
});

describe("quota", () => {
  it("grades each window and says when it resets", () => {
    const r = read(quota(92, 1000 + 2 * 3600), 1_060_000);
    expect(r.stale).toBeNull();
    expect(r.windows).toEqual([
      { label: "5h", used: 92, level: "full", resets: "resets in 1h" },
      { label: "week · opus", used: 10, level: "ok", resets: null },
    ]);
    expect(read(quota(80, 9999), 1_060_000).windows[0].level).toBe("low");
  });

  it("says an old reading's age, and a window past its reset as reset", () => {
    const r = read(quota(92, 2000), 4_600_000);
    expect(r.stale).toBe("1h ago");
    expect(r.windows[0]).toEqual({ label: "5h", used: 0, level: "reset", resets: null });
  });

  it("keeps labels short", () => {
    expect(label("seven_day")).toBe("week");
    expect(label("seven_day_oauth_apps")).toBe("week · oauth apps");
    expect(label("daily")).toBe("daily");
  });
});
