// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CohortResponse } from "@/types/metricTree";
import { WorldModelPeerCohortSection } from "./WorldModelPeerCohortSection";

// The section reaches the API only through these two hooks.
const useCohortQuery = vi.hoisted(() => vi.fn());
const useTimeDimensions = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/api/useMetricTree", () => ({
  useCohortQuery,
  useTimeDimensions
}));

const MEASURE = "sales.wage_pct";

/** The instance in focus. `S1` is the subject the panel must pick out of a
 *  population-wide response. */
const INSTANCE = { entity: "restaurant", key: "S1" };

function response(over: Partial<CohortResponse> = {}): CohortResponse {
  return {
    entity: "restaurant",
    cohort: "size_matched",
    measure: MEASURE,
    statistic: "median",
    period: ["2026-04-15", "2026-07-13"],
    band_window: ["2026-01-15", "2026-07-13"],
    subjects: [
      {
        key: "S1",
        value: 31.5,
        baseline: 26.0,
        gap: 5.5,
        peers: ["S2", "S7", "S9"],
        peer_count: 3,
        sufficient: true
      },
      {
        key: "S2",
        value: 26.0,
        baseline: 27.1,
        gap: -1.1,
        peers: ["S1"],
        peer_count: 1,
        sufficient: false
      }
    ],
    excluded: [],
    ...over
  };
}

function renderSection(
  { data, error }: { data?: CohortResponse; error?: unknown } = { data: response() },
  instance = INSTANCE
) {
  useTimeDimensions.mockReturnValue({
    data: { by_view: { sales: ["sales.business_date"] } },
    isPending: false
  });
  useCohortQuery.mockReturnValue({ data, error: error ?? null, isPending: false });

  return render(
    <WorldModelPeerCohortSection
      measure={MEASURE}
      view='sales'
      instance={instance}
      periodDays={90}
    />
  );
}

function expand() {
  fireEvent.click(screen.getByTestId(`wm-peers-toggle-${MEASURE}`));
}

beforeEach(() => {
  useTimeDimensions.mockReturnValue({ data: undefined, isPending: false });
  useCohortQuery.mockReturnValue({ data: undefined, error: null, isPending: false });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WorldModelPeerCohortSection", () => {
  it("is a separate section from Opportunities, labelled by the comparison it makes", () => {
    // The whole reason this is not merged into the opportunities list: a domain
    // expert read that list as "vs other restaurants" when it is "vs this
    // restaurant's own channels". Each section must say which comparison it is.
    renderSection();
    const toggle = screen.getByTestId(`wm-peers-toggle-${MEASURE}`);
    expect(toggle.textContent).toContain("Compared to peers");
    expect(toggle.textContent).toMatch(/other restaurant/i);
  });

  it("costs nothing until expanded", () => {
    // A cohort resolution is two entity-grain warehouse pulls per measure, and
    // an instance panel mounts one of these per computed measure. Unlike the
    // opportunity scan next door it is lazy, so the query stays disabled while
    // the section is shut.
    renderSection();
    expect(useCohortQuery).toHaveBeenCalledWith(null, false);

    expand();
    const [request, enabled] = useCohortQuery.mock.calls.at(-1) as [unknown, boolean];
    expect(enabled).toBe(true);
    expect(request).toMatchObject({
      entity: "restaurant",
      measure: MEASURE,
      time_dimension: "sales.business_date"
    });
  });

  it("names the server's authoring fault when the declared cohort does not resolve", () => {
    // The section mounts only where the measure declares a `default_cohort:` on
    // this entity, so `cohort_not_resolved` now means a declaration the server
    // could not use — today, one not in `entity.cohort_name` form. Telling the
    // modeller to add a declaration they already wrote hides the real fault;
    // the server's message names it. Still an authoring gap, not a failure.
    renderSection({
      error: {
        code: "cohort_not_resolved",
        message:
          "measure `sales.wage_pct` declares `default_cohort: size_matched`, which is not in `entity.cohort_name` form"
      }
    });
    expand();

    const undeclared = screen.getByTestId("wm-peers-undeclared");
    expect(undeclared.textContent).toContain("not in `entity.cohort_name` form");
    expect(undeclared.textContent).not.toMatch(/no peer cohort is declared/i);
    expect(undeclared.textContent).not.toContain("Add a cohorts:");
    expect(screen.queryByTestId("wm-peers-error")).toBeNull();
  });

  it("still shows a real failure as a failure", () => {
    renderSection({ error: new Error("warehouse exploded") });
    expand();
    expect(screen.getByTestId("wm-peers-error").textContent).toContain("warehouse exploded");
    expect(screen.queryByTestId("wm-peers-undeclared")).toBeNull();
  });

  it("shows a failed time-dimension lookup as a failure, not as a modelling gap", () => {
    // The cohort query waits on a time dimension, so when discovery fails it
    // never runs and its own error display can never surface the cause. Saying
    // "no date dimension is declared" would send the modeller to fix a view
    // that is fine.
    renderSection();
    useTimeDimensions.mockReturnValue({
      data: undefined,
      isPending: false,
      error: new Error("time dimensions unavailable")
    });
    expand();

    expect(screen.getByTestId("wm-peers-error").textContent).toContain(
      "time dimensions unavailable"
    );
    expect(screen.queryByText(/No date or datetime dimension/)).toBeNull();
  });

  it("says no date dimension is declared only when discovery found none", () => {
    renderSection();
    useTimeDimensions.mockReturnValue({
      data: { by_view: { sales: [] } },
      isPending: false,
      error: null
    });
    expand();

    expect(screen.getByText(/No date or datetime dimension/)).toBeTruthy();
    expect(screen.queryByTestId("wm-peers-error")).toBeNull();
  });

  it("picks this instance out of the population and shows its peer count", () => {
    // The response carries a row for every subject; the panel is about one.
    renderSection();
    expand();

    const self = screen.getByTestId("wm-peers-self");
    expect(self.textContent).toContain("31.5");
    expect(self.textContent).toContain("26.0");
    // The peer count is what makes a baseline readable — never just the number.
    expect(self.textContent).toContain("3");
    // Another subject's row is not this instance's business.
    expect(screen.queryByTestId("wm-peers-self")?.textContent).not.toContain("S2");
  });

  it("echoes the cohort and band window that produced the numbers", () => {
    // So what is on screen cannot drift from the query behind it.
    renderSection();
    expand();

    const provenance = screen.getByTestId("wm-peers-provenance");
    expect(provenance.textContent).toContain("size_matched");
    expect(provenance.textContent).toContain("median");
    expect(provenance.textContent).toContain("2026-01-15");
  });

  it("names the time dimension the period was filtered on", () => {
    // The Opportunities section above lets the reader pick another time
    // dimension; this one compares over the view's first. Without naming it,
    // the two could describe different columns while looking alike.
    renderSection();
    expand();

    expect(screen.getByTestId("wm-peers-provenance").textContent).toContain("sales.business_date");
  });

  it("flags a thin peer set instead of hiding the comparison", () => {
    // `min_peers` is a reporting predicate, never a filter: an insufficient
    // subject arrives with its comparison computed, and hiding it would throw
    // away the only signal the reader has about how much to trust it.
    renderSection({
      data: response({
        subjects: [
          {
            key: "S1",
            value: 31.5,
            baseline: 26.0,
            gap: 5.5,
            peers: ["S2"],
            peer_count: 1,
            sufficient: false
          }
        ]
      })
    });
    expand();

    expect(screen.getByTestId("wm-peers-self")).toBeTruthy();
    expect(screen.getByTestId("wm-peers-thin").textContent).toMatch(/1 peer/i);
  });

  it("says why an excluded subject was left out", () => {
    // The failure this replaces: a store simply did not appear in the list and
    // no screen said why.
    renderSection({
      data: response({
        excluded: [
          { key: "S4", reason: "opened mid-period" },
          { key: "(null) [direct_only]", reason: "entity key was NULL" }
        ]
      })
    });
    expand();

    const excluded = screen.getByTestId("wm-peers-excluded");
    expect(excluded.textContent).toContain("S4");
    expect(excluded.textContent).toContain("opened mid-period");
    expect(excluded.textContent).toContain("(null) [direct_only]");
    expect(excluded.textContent).toContain("entity key was NULL");
  });

  it("says so when THIS instance is the excluded one, rather than rendering empty", () => {
    // The key may be in `excluded` instead of `subjects`. Rendering nothing here
    // is the exact bug the reason strings exist to prevent.
    renderSection({
      data: response({
        subjects: [],
        excluded: [{ key: "S1", reason: "no matching peers within the size band" }]
      })
    });
    expand();

    const self = screen.getByTestId("wm-peers-self-excluded");
    expect(self.textContent).toContain("no matching peers within the size band");
    expect(screen.queryByTestId("wm-peers-self")).toBeNull();
  });

  it("says so when the instance is in neither list", () => {
    renderSection({ data: response({ subjects: [], excluded: [] }) });
    expand();
    expect(screen.getByTestId("wm-peers-self-missing")).toBeTruthy();
  });

  it("matches a single-column key the world model spells as a JSON array", () => {
    // The world-model API addresses an instance key as a JSON array for a
    // composite key and a bare scalar otherwise; airlayer keys a subject by the
    // value. A one-element array is the same instance and must match.
    renderSection({ data: response() }, { entity: "restaurant", key: '["S1"]' });
    expand();
    expect(screen.getByTestId("wm-peers-self").textContent).toContain("31.5");
  });
});
