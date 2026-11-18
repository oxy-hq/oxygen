import { describe, expect, it } from "vitest";
import type { AppEventListenerHealth, AppEventType } from "@/services/api/appEvents";
import { eventsView, filterEventRows, formatWait, sumCounts } from "./eventsView";

const listener = (
  app: string,
  fn: string,
  environment: string,
  counts: Partial<AppEventListenerHealth> = {}
): AppEventListenerHealth => ({
  listener: `${app}/${fn}`,
  app,
  function: fn,
  environment,
  pending: 0,
  retrying: 0,
  dead: 0,
  held: 0,
  oldestPendingAgeSeconds: null,
  lastSuccessAt: null,
  lastFailureAt: null,
  lastFailureOutcome: null,
  inFlight: 0,
  buildId: null,
  reconciledAt: null,
  ...counts
});

const type = (
  publisher: string,
  event: string,
  listeners: AppEventListenerHealth[]
): AppEventType => ({
  type: `${publisher}.${event}`,
  publisher,
  event,
  schemaRevision: 1,
  lastPublishedAt: null,
  listeners
});

describe("eventsView", () => {
  it("leaves out types the app neither publishes nor listens to", () => {
    const view = eventsView(
      [type("billing", "paid", [listener("crm", "onPaid", "production")])],
      "store",
      "production"
    );
    expect(view.rows).toEqual([]);
  });

  it("counts every listener of a type the app publishes, whichever app it is in", () => {
    const view = eventsView(
      [
        type("store", "ordered", [
          listener("store", "ship", "production", { pending: 2 }),
          listener("crm", "note", "production", { dead: 3 })
        ])
      ],
      "store",
      "production"
    );
    expect(view.rows).toHaveLength(1);
    expect(view.rows[0].role).toBe("both");
    expect(view.rows[0].counts.pending).toBe(2);
    expect(view.rows[0].counts.dead).toBe(3);
  });

  it("counts only the app's own listeners on a type another app publishes", () => {
    const view = eventsView(
      [
        type("billing", "paid", [
          listener("store", "onPaid", "production", { pending: 1 }),
          listener("crm", "onPaid", "production", { dead: 9 })
        ])
      ],
      "store",
      "production"
    );
    expect(view.rows[0].role).toBe("listens");
    expect(view.rows[0].listeners.map((l) => l.listener)).toEqual(["store/onPaid"]);
    expect(view.rows[0].counts.dead).toBe(0);
  });

  it("counts only the environment the page shows", () => {
    const view = eventsView(
      [
        type("store", "ordered", [
          listener("store", "ship", "production", { pending: 1 }),
          listener("store", "ship", "staging", { dead: 5 })
        ])
      ],
      "store",
      "production"
    );
    expect(view.totals.dead).toBe(0);
    expect(view.totals.pending).toBe(1);
  });

  it("drops a type the app only listens to in another environment", () => {
    const view = eventsView(
      [type("billing", "paid", [listener("store", "onPaid", "staging", { dead: 1 })])],
      "store",
      "production"
    );
    expect(view.rows).toEqual([]);
  });

  it("keeps a type the app publishes even when nothing listens in this environment", () => {
    const view = eventsView([type("store", "ordered", [])], "store", "production");
    expect(view.rows).toHaveLength(1);
    expect(view.rows[0].role).toBe("publishes");
    expect(view.rows[0].listeners).toEqual([]);
  });

  it("puts the type with dead deliveries first, then held, then the longest wait", () => {
    const view = eventsView(
      [
        type("store", "a", [
          listener("store", "x", "production", { oldestPendingAgeSeconds: 900, pending: 1 })
        ]),
        type("store", "b", [listener("store", "x", "production", { held: 1 })]),
        type("store", "c", [listener("store", "x", "production", { dead: 1 })]),
        type("store", "d", [
          listener("store", "x", "production", { oldestPendingAgeSeconds: 30, pending: 1 })
        ])
      ],
      "store",
      "production"
    );
    expect(view.rows.map((r) => r.event)).toEqual(["c", "b", "a", "d"]);
  });

  it("totals the rows, keeping the longest wait rather than adding waits", () => {
    const view = eventsView(
      [
        type("store", "a", [
          listener("store", "x", "production", { pending: 1, oldestPendingAgeSeconds: 40 })
        ]),
        type("store", "b", [
          listener("store", "y", "production", { pending: 2, oldestPendingAgeSeconds: 600 })
        ])
      ],
      "store",
      "production"
    );
    expect(view.totals.pending).toBe(3);
    expect(view.totals.oldestPendingSeconds).toBe(600);
  });
});

describe("sumCounts", () => {
  it("has no wait when nothing is pending", () => {
    expect(sumCounts([]).oldestPendingSeconds).toBeNull();
  });
});

describe("filterEventRows", () => {
  const view = eventsView(
    [
      type("store", "ordered", [listener("store", "ship", "production", { dead: 1 })]),
      type("store", "refunded", [listener("crm", "note", "production", { pending: 2 })]),
      type("store", "viewed", [])
    ],
    "store",
    "production"
  );

  it("matches a type or one of its listeners by name", () => {
    expect(
      filterEventRows(view.rows, { query: "REFUND", pendingOnly: false, deadOnly: false })
    ).toHaveLength(1);
    expect(
      filterEventRows(view.rows, { query: "crm/note", pendingOnly: false, deadOnly: false })
    ).toHaveLength(1);
  });

  it("narrows to types with dead deliveries, or with pending ones", () => {
    expect(
      filterEventRows(view.rows, { query: "", pendingOnly: false, deadOnly: true }).map(
        (r) => r.event
      )
    ).toEqual(["ordered"]);
    expect(
      filterEventRows(view.rows, { query: "", pendingOnly: true, deadOnly: false }).map(
        (r) => r.event
      )
    ).toEqual(["refunded"]);
  });

  it("keeps a type only when it satisfies both toggles", () => {
    expect(filterEventRows(view.rows, { query: "", pendingOnly: true, deadOnly: true })).toEqual(
      []
    );
  });
});

describe("formatWait", () => {
  it("rounds to the largest unit that fits", () => {
    expect(formatWait(null)).toBeNull();
    expect(formatWait(12.4)).toBe("12s");
    expect(formatWait(600)).toBe("10m");
    expect(formatWait(3 * 3600)).toBe("3h");
    expect(formatWait(2 * 86_400)).toBe("2d");
  });
});
