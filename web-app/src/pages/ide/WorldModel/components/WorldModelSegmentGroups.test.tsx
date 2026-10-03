// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, type Mock, vi } from "vitest";
import type { DimensionOpportunity } from "@/types/metricTree";
import type { WorldModel } from "@/types/worldModel";
import { SizingBody } from "./WorldModelSegmentGroups";

// The rows reach the API only through the per-row drill (useDrillQuery); the
// other three are imported by modules in this file's import graph, so the mock
// factory has to carry them or the import itself fails.
const useDrillQuery = vi.hoisted(() => vi.fn());
const useMetricTree = vi.hoisted(() => vi.fn());
const useOpportunityQuery = vi.hoisted(() => vi.fn());
const useTimeDimensions = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/api/useMetricTree", () => ({
  useDrillQuery,
  useMetricTree,
  useOpportunityQuery,
  useTimeDimensions
}));

const EMPTY_MODEL: WorldModel = { entities: [], edges: [] };

/** One dimension with two ranked segments, so "expand rows independently" has
 *  a second row to expand. */
const CHANNEL: DimensionOpportunity = {
  dimension: "orders.channel",
  cardinality: 4,
  benchmark_basis: "p75",
  total_upside: 12000,
  other_segments_skipped: 0,
  segments_dropped_as_noise: 0,
  segments: [
    { segment: "mobile_app", current_value: 21, volume: 400, benchmark: 30, gap: 9, upside: 3600 },
    { segment: "web", current_value: 25, volume: 300, benchmark: 30, gap: 5, upside: 1500 }
  ]
};

function renderSizingBody({ drillEnabled }: { drillEnabled: boolean }) {
  useDrillQuery.mockReturnValue({ data: undefined, isPending: false, error: null });
  const result = render(
    <SizingBody
      dimensions={[CHANNEL]}
      target='orders.revenue'
      view='orders'
      periodDays={90}
      overallValue={100000}
      model={EMPTY_MODEL}
      onSelect={vi.fn()}
      timeDimension='orders.created_at'
      drillEnabled={drillEnabled}
    />
  );
  // Segment rows live under a collapsed dimension group; open it so the rows
  // (and their drill affordance) are on screen.
  fireEvent.click(screen.getByTestId("wm-opp-dim-orders.channel"));
  return result;
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("SizingBody", () => {
  it("expands a segment row into its own chain", () => {
    // The ranked row IS drill level 0: expanding it roots the engine's
    // decomposition at that row rather than at the engine's own top pick.
    renderSizingBody({ drillEnabled: true });
    fireEvent.click(screen.getByTestId("wm-opp-drill-toggle-mobile_app"));
    // The drill's target is the sizing `target` itself — there is no separate
    // node id to pass, so a row can't be decomposed against a different measure
    // than the one it was sized on.
    expect(useDrillQuery).toHaveBeenCalledWith(
      expect.objectContaining({
        target: "orders.revenue",
        root: { dimension: "orders.channel", segment: "mobile_app" }
      }),
      true
    );
  });

  it("expands rows independently", () => {
    // Expansion state is per row, so opening a second row must not collapse the
    // first — every cut is actionable, not just the engine's pick.
    renderSizingBody({ drillEnabled: true });
    fireEvent.click(screen.getByTestId("wm-opp-drill-toggle-mobile_app"));
    fireEvent.click(screen.getByTestId("wm-opp-drill-toggle-web"));
    const roots = (useDrillQuery as Mock).mock.calls.map(([req]) => req?.root?.segment);
    expect(roots).toContain("mobile_app");
    expect(roots).toContain("web");
  });

  it("offers no drill affordance when the measure cannot be decomposed", () => {
    // The section above owns the gate; a row must not offer a chain the engine
    // would refuse.
    renderSizingBody({ drillEnabled: false });
    expect(screen.queryByTestId("wm-opp-drill-toggle-mobile_app")).toBeNull();
  });

  it("fetches no chain until a row is expanded", () => {
    // A dimension can hold five rows; mounting them must cost nothing. The
    // recursive drill is a bounded but real number of warehouse queries.
    renderSizingBody({ drillEnabled: true });
    expect(useDrillQuery).not.toHaveBeenCalled();
  });

  it("marks the engine's own top pick without expanding it", () => {
    // The first segment of the first dimension is the row an unrooted drill
    // would have followed. It stays visible as a recommendation — marked, not
    // privileged, and not auto-opened.
    renderSizingBody({ drillEnabled: true });
    expect(screen.getByText("top pick")).toBeTruthy();
    expect(useDrillQuery).not.toHaveBeenCalled();
  });
});
