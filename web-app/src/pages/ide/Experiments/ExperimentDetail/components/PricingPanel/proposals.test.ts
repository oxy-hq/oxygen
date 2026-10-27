import { describe, expect, it } from "vitest";
import { failed, pending, proposal, refused } from "./fixtures";
import {
  assignmentsProposed,
  newestProposal,
  PRICING_TIMEOUT_MS,
  pricingView,
  requestProposals
} from "./proposals";

const base = { now: 1_000, timeoutMs: PRICING_TIMEOUT_MS };
const awaiting = { requestId: "new", ids: ["n1", "n2"], startedAt: 0 };
const inNew = { request_id: "new" };

describe("pricingView", () => {
  it("is idle with nothing awaited and an empty log", () => {
    expect(pricingView({ ...base, awaiting: null, proposals: [] })).toEqual({
      status: "idle",
      proposals: [],
      total: 0,
      settled: 0
    });
  });

  it("shows the newest request when nothing is awaited, whatever its candidates' statuses", () => {
    const log = [
      proposal({ id: "a", request_id: "r2", created_at: "2026-09-30T00:00:00Z" }),
      refused("b", "no", { request_id: "r2", created_at: "2026-09-30T00:00:01Z" }),
      proposal({ id: "c", request_id: "r1", created_at: "2026-09-01T00:00:00Z" })
    ];
    const view = pricingView({ ...base, awaiting: null, proposals: log });
    expect(view.status).toBe("settled");
    expect(view.proposals.map((p) => p.id)).toEqual(["a", "b"]);
  });

  it("resumes waiting on a request with a pending candidate it did not queue (a reload mid-job)", () => {
    const view = pricingView({
      ...base,
      awaiting: null,
      proposals: [proposal({ id: "a", ...inNew }), pending("b", inNew)]
    });
    expect(view).toMatchObject({ status: "waiting", total: 2, settled: 1 });
  });

  it("stops waiting on a resumed request once the timeout has passed since the page began waiting", () => {
    const log = [proposal({ id: "a", ...inNew }), pending("b", inNew)];
    const since = 5_000;
    const resumed = {
      awaiting: null,
      proposals: log,
      resumedAt: since,
      timeoutMs: PRICING_TIMEOUT_MS
    };
    expect(pricingView({ ...resumed, now: since + PRICING_TIMEOUT_MS - 1 }).status).toBe("waiting");
    expect(pricingView({ ...resumed, now: since + PRICING_TIMEOUT_MS })).toMatchObject({
      status: "timed_out",
      total: 2,
      settled: 1
    });
    // The answer still beats the clock.
    expect(
      pricingView({
        ...resumed,
        proposals: [proposal({ id: "a", ...inNew }), proposal({ id: "b", ...inNew })],
        now: since + PRICING_TIMEOUT_MS
      }).status
    ).toBe("settled");
  });

  it("waits until EVERY candidate of the awaited request has settled, counting progress", () => {
    const one = pricingView({
      ...base,
      awaiting,
      proposals: [proposal({ id: "n1", ...inNew }), pending("n2", inNew)]
    });
    expect(one).toMatchObject({ status: "waiting", total: 2, settled: 1 });

    const both = pricingView({
      ...base,
      awaiting,
      proposals: [proposal({ id: "n1", ...inNew }), failed("n2", "faulted", inNew)]
    });
    expect(both).toMatchObject({ status: "settled", total: 2, settled: 2 });
  });

  it("waits while rows of the awaited request are missing, and ignores other requests' results", () => {
    const view = pricingView({
      ...base,
      awaiting,
      proposals: [proposal({ id: "old", request_id: "old" })]
    });
    expect(view).toEqual({ status: "waiting", proposals: [], total: 2, settled: 0 });
  });

  it.each(["done", "refused", "failed"] as const)(
    "a candidate that settles as %s counts as settled",
    (status) => {
      const row =
        status === "done"
          ? proposal({ id: "n1", ...inNew })
          : status === "refused"
            ? refused("n1", "no", inNew)
            : failed("n1", "no", inNew);
      const view = pricingView({
        ...base,
        awaiting: { ...awaiting, ids: ["n1"] },
        proposals: [row]
      });
      expect(view.status).toBe("settled");
    }
  );

  it("times out only while a candidate of the awaited request is still absent or pending", () => {
    const late = { now: PRICING_TIMEOUT_MS + 1, timeoutMs: PRICING_TIMEOUT_MS };
    expect(
      pricingView({
        ...late,
        awaiting,
        proposals: [proposal({ id: "n1", ...inNew }), pending("n2", inNew)]
      })
    ).toMatchObject({ status: "timed_out", settled: 1 });
    expect(pricingView({ ...late, awaiting, proposals: [] }).status).toBe("timed_out");
  });

  it("still reports a request that settles after the deadline", () => {
    expect(
      pricingView({
        now: PRICING_TIMEOUT_MS + 5_000,
        timeoutMs: PRICING_TIMEOUT_MS,
        awaiting,
        proposals: [refused("n1", "no", inNew), refused("n2", "no", inNew)]
      }).status
    ).toBe("settled");
  });
});

describe("requestProposals", () => {
  it("keeps one request's candidates in the order they were created", () => {
    const log = [
      proposal({ id: "b", request_id: "r", created_at: "2026-09-30T00:00:02Z" }),
      proposal({ id: "x", request_id: "other" }),
      proposal({ id: "a", request_id: "r", created_at: "2026-09-30T00:00:01Z" })
    ];
    expect(requestProposals(log, "r").map((p) => p.id)).toEqual(["a", "b"]);
  });

  it("breaks a created_at tie by id, so the order is the same on every read", () => {
    const log = [proposal({ id: "b", request_id: "r" }), proposal({ id: "a", request_id: "r" })];
    expect(requestProposals(log, "r").map((p) => p.id)).toEqual(["a", "b"]);
  });
});

describe("newestProposal", () => {
  it("picks the latest by created_at whatever order the server sent", () => {
    const list = [
      proposal({ id: "old", created_at: "2026-09-01T00:00:00Z" }),
      proposal({ id: "new", created_at: "2026-09-30T00:00:00Z" }),
      proposal({ id: "mid", created_at: "2026-09-15T00:00:00Z" })
    ];
    expect(newestProposal(list)?.id).toBe("new");
    expect(newestProposal([])).toBeNull();
  });
});

describe("assignmentsProposed", () => {
  it("counts done candidates only — a refusal or a fault assigns nothing", () => {
    const list = [
      proposal({ id: "a" }),
      refused("b", "no"),
      failed("c", "no"),
      pending("d"),
      proposal({ id: "e" })
    ];
    expect(assignmentsProposed(list)).toBe(2);
    expect(assignmentsProposed([])).toBe(0);
  });
});
