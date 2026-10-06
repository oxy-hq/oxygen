import { describe, expect, it } from "vitest";
import type { ApiKeyActivityEvent } from "@/types/apiKey";
import { expiryChange, fillUsageDays, lifecycleLabel, splitEvents, usageTotals } from "./activity";

const event = (over: Partial<ApiKeyActivityEvent>): ApiKeyActivityEvent => ({
  id: over.action ?? "e",
  created_at: "2026-09-30T10:00:00Z",
  actor_email: "a@example.com",
  actor_type: "user",
  action: "token.created",
  org_id: null,
  workspace_id: null,
  partner_id: null,
  target_type: null,
  target_id: null,
  target_label: null,
  outcome: "success",
  reason: null,
  via_global_override: false,
  ...over
});

describe("splitEvents", () => {
  it("separates what was done to the key from what was done with it, keeping order", () => {
    const events = [
      event({ id: "1", action: "token.extended" }),
      event({ id: "2", action: "secret.created", actor_type: "api_key" }),
      event({ id: "3", action: "token.created" })
    ];
    const { lifecycle, actions } = splitEvents(events);
    expect(lifecycle.map((e) => e.id)).toEqual(["1", "3"]);
    expect(actions.map((e) => e.id)).toEqual(["2"]);
  });
});

describe("lifecycleLabel", () => {
  it("names known events and degrades readably for new ones", () => {
    expect(lifecycleLabel("token.extended")).toBe("Extended");
    expect(lifecycleLabel("token.grants_changed")).toBe("Grants changed");
  });
});

describe("expiryChange", () => {
  it("reads the old and new expiry, with null meaning no expiry", () => {
    const e = event({
      action: "token.extended",
      metadata: { old_expires_at: "2026-10-03T00:00:00Z", new_expires_at: null }
    });
    expect(expiryChange(e)).toEqual({ from: "2026-10-03T00:00:00Z", to: null });
  });

  it("shows nothing when the server sent no metadata (the /admin/audit row shape)", () => {
    expect(expiryChange(event({ action: "token.extended" }))).toBeNull();
    expect(
      expiryChange(event({ action: "token.extended", metadata: { token_id: "x" } }))
    ).toBeNull();
  });
});

describe("fillUsageDays", () => {
  const now = new Date("2026-09-30T18:00:00Z");

  it("returns 30 consecutive days ending today, zero-filling omitted days", () => {
    const days = fillUsageDays(
      [{ day: "2026-09-29", requests: 5, errors_4xx: 1, errors_5xx: 0 }],
      now
    );
    expect(days).toHaveLength(30);
    expect(days[0].day).toBe("2026-09-01");
    expect(days[29]).toEqual({ day: "2026-09-30", requests: 0, errors_4xx: 0, errors_5xx: 0 });
    expect(days[28].requests).toBe(5);
  });

  it("ends at the server's newest day when its clock is ahead of ours", () => {
    const days = fillUsageDays(
      [{ day: "2026-10-01", requests: 2, errors_4xx: 0, errors_5xx: 0 }],
      now
    );
    expect(days[29].day).toBe("2026-10-01");
    expect(days[29].requests).toBe(2);
  });
});

describe("usageTotals", () => {
  it("never counts a negative OK, even if the server's error counts exceed requests", () => {
    const totals = usageTotals([
      { day: "2026-09-29", requests: 10, errors_4xx: 2, errors_5xx: 1 },
      { day: "2026-09-30", requests: 1, errors_4xx: 2, errors_5xx: 0 }
    ]);
    expect(totals).toEqual({ requests: 11, ok: 7, errors4xx: 4, errors5xx: 1 });
  });
});
