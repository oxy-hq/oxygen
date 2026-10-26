// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OpportunityInstance } from "@/types/metricTree";
import type { WorldModel } from "@/types/worldModel";
import { WorldModelInstanceOpportunity } from "./WorldModelInstanceOpportunity";

// The merged section reaches the API only through these hooks, so mocking them
// here covers every request it can make — no QueryClientProvider needed.
const useMetricTree = vi.hoisted(() => vi.fn());
const useOpportunityQuery = vi.hoisted(() => vi.fn());
const useTimeDimensions = vi.hoisted(() => vi.fn());
const useDrillQuery = vi.hoisted(() => vi.fn());
const useCohortQuery = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/api/useMetricTree", () => ({
  useMetricTree,
  useOpportunityQuery,
  useTimeDimensions,
  useDrillQuery,
  useCohortQuery
}));

const EMPTY_MODEL: WorldModel = { entities: [], edges: [] };
const INSTANCE: OpportunityInstance = { entity: "store", key: "S1" };

/** A sized (rows-mode) response, so the eager `hideWhenEmpty` gate lets the
 *  toggle render. `undefined` models "the engine had nothing to size". */
const SIZED = {
  target: "checks.net_revenue",
  period: ["2026-04-15", "2026-07-13"],
  overall_value: 100000,
  weight_basis: "rows",
  skipped_dimensions: [],
  downstream: [],
  dimensions: [
    {
      dimension: "checks.channel",
      cardinality: 4,
      benchmark_basis: "p75",
      total_upside: 12000,
      other_segments_skipped: 0,
      segments_dropped_as_noise: 0,
      segments: [
        {
          segment: "mobile_app",
          current_value: 21,
          volume: 400,
          benchmark: 30,
          gap: 9,
          upside: 3600
        }
      ]
    }
  ]
};

function renderInstanceOpportunity(
  additivity: string,
  drillable: boolean,
  opp: unknown = SIZED,
  // Declared by default: every case below predates the gate and is about the
  // sizing half. `null` is the workspace that has authored no `cohorts:` —
  // spelled `null` and not `undefined` because an explicit `undefined` argument
  // re-applies this very default.
  defaultCohort: string | null = "store.size_matched",
  cohort: unknown = { data: undefined, isPending: false, error: null }
) {
  useMetricTree.mockReturnValue({
    data: {
      nodes: [
        {
          id: "checks.net_revenue",
          view: "checks",
          measure: "net_revenue",
          label: "net_revenue",
          measure_type: "custom",
          is_composite: true,
          drillable
        }
      ],
      edges: []
    }
  });
  useTimeDimensions.mockReturnValue({
    data: { by_view: { checks: ["checks.created_at"] } },
    isPending: false
  });
  useOpportunityQuery.mockReturnValue({ data: opp, isPending: false, error: null });
  useDrillQuery.mockReturnValue({ data: undefined, isPending: false, error: null });
  useCohortQuery.mockReturnValue(cohort);

  return render(
    <WorldModelInstanceOpportunity
      measureName='net_revenue'
      induced={false}
      promotedFrom={undefined}
      entityView='checks'
      additivity={additivity}
      instance={INSTANCE}
      model={EMPTY_MODEL}
      onSelect={vi.fn()}
      defaultCohort={defaultCohort}
    />
  );
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WorldModelInstanceOpportunity", () => {
  it("keeps the decomposition affordance for a drillable passthrough composite", () => {
    // `checks.net_revenue` is an engine-accepted additive composite. airlayer
    // classifies it Passthrough (Number|Custom -> Passthrough), NOT Additive —
    // so a bare `sizable` (additive | non_additive) gate returns null before the
    // section ever mounts, killing the affordance on exactly the measures this
    // whole feature exists to expose. Post-merge there is one section, so the
    // gate must be `sizable || drillEnabled` and the section's own query gate
    // must admit a drillable node too.
    //
    // Asserting the collapsed chevron alone is not enough: that passes even if
    // the section opens onto "No addressable upside to size" with no drill
    // affordance anywhere. Expand it and check the real per-row decomposition.
    renderInstanceOpportunity("passthrough", true);

    expect(screen.getByTestId("wm-opp-toggle-checks.net_revenue")).toBeDefined();
    fireEvent.click(screen.getByTestId("wm-opp-toggle-checks.net_revenue"));
    fireEvent.click(screen.getByTestId("wm-opp-dim-checks.channel"));
    expect(screen.getByTestId("wm-opp-drill-toggle-mobile_app")).toBeDefined();
  });

  it("hides sizing for a passthrough the engine does not accept, but still offers peers", () => {
    // A passthrough the engine refuses (nested/cross-view/multiplicative refs)
    // is neither sizable nor drillable, so the Opportunities half stays away —
    // that gate is unchanged.
    //
    // Peer comparison is a DIFFERENT predicate: whether the entity declares a
    // `cohorts:` block and the measure a `default_cohort:`. Inheriting the
    // sizing gate would hide the comparison for measures that declare one
    // perfectly well, so the peer section is offered regardless.
    renderInstanceOpportunity("passthrough", false);
    expect(screen.queryByTestId("wm-opp-toggle-checks.net_revenue")).toBeNull();
    expect(screen.getByTestId("wm-peers-toggle-checks.net_revenue")).toBeTruthy();
  });

  it("offers no peer toggle for a measure that declares no cohort", () => {
    // The toggle used to render unconditionally, so on a workspace that has
    // authored no `cohorts:` — every workspace today — clicking it spent a
    // request only to print "no peer cohort is declared for this measure".
    // `default_cohort` now rides the world-model graph (schema-only, no query),
    // so the affordance appears where a comparison actually exists. The sizing
    // half is unaffected: the two gates are independent predicates.
    renderInstanceOpportunity("additive", true, SIZED, null);

    expect(screen.queryByTestId("wm-peers-toggle-checks.net_revenue")).toBeNull();
    expect(screen.getByTestId("wm-opp-toggle-checks.net_revenue")).toBeDefined();
  });

  it("offers no peer toggle on an entity the measure's cohort is not declared on", () => {
    // One measure is placed on several entities: its view's primary entity
    // owns it and every ancestor it is promoted to lists it as induced, each
    // carrying the same `default_cohort`. Only the entity the declaration
    // names can resolve it — the server refuses every other one with
    // `cohort_not_resolved`, whose copy would tell a correctly-authored model
    // to re-point its cohort. So the toggle stays on the declaring entity.
    renderInstanceOpportunity("additive", true, SIZED, "restaurant.size_matched");

    expect(screen.queryByTestId("wm-peers-toggle-checks.net_revenue")).toBeNull();
    expect(screen.getByTestId("wm-opp-toggle-checks.net_revenue")).toBeDefined();
  });

  it("keeps the peer toggle for a declaration not in `entity.cohort` form", () => {
    // A bare `default_cohort: size_matched` names no entity, so it cannot be
    // ruled out as belonging elsewhere. The server refuses it as malformed —
    // an authoring error the reader must be shown, not a toggle to hide.
    renderInstanceOpportunity("additive", true, SIZED, "size_matched");

    expect(screen.getByTestId("wm-peers-toggle-checks.net_revenue")).toBeTruthy();
  });

  it("still renders the undeclared-cohort copy when a declared cohort fails to resolve", () => {
    // The gate is the measure's DECLARATION, not the resolution: a measure can
    // declare `default_cohort:` on its own entity pointing at a `cohorts:`
    // block that is missing. That is a real authoring error and must keep its
    // explanatory copy rather than vanishing with the toggle.
    renderInstanceOpportunity("additive", true, SIZED, "store.size_matched", {
      data: undefined,
      isPending: false,
      error: { code: "cohort_not_resolved", message: "declares no `default_cohort:`" }
    });

    fireEvent.click(screen.getByTestId("wm-peers-toggle-checks.net_revenue"));
    expect(screen.getByTestId("wm-peers-undeclared")).toBeTruthy();
  });

  it("keeps the two comparisons in separate, separately-labelled sections", () => {
    // The misreading this feature answers: the Opportunities list benchmarks
    // this instance's own dimension segments against each other, and was read
    // as a comparison against other restaurants. Both sections are present and
    // each says which comparison it made — never one merged "upside" list.
    renderInstanceOpportunity("additive", true);

    const sizing = screen.getByTestId("wm-opp-toggle-checks.net_revenue");
    const peers = screen.getByTestId("wm-peers-toggle-checks.net_revenue");
    expect(sizing.textContent).toContain("Opportunities");
    expect(peers.textContent).toContain("Compared to peers");
    expect(peers.textContent).toMatch(/other stores/i);
  });

  it("does not resolve a cohort until the peer section is expanded", () => {
    // One cohort resolution is two entity-grain warehouse pulls, and the panel
    // mounts one of these per computed measure. Unlike the eager sizing scan it
    // must stay disabled while collapsed.
    renderInstanceOpportunity("additive", true);
    expect(useCohortQuery).toHaveBeenCalledWith(null, false);

    fireEvent.click(screen.getByTestId("wm-peers-toggle-checks.net_revenue"));
    const [, enabled] = useCohortQuery.mock.calls.at(-1) as [unknown, boolean];
    expect(enabled).toBe(true);
  });

  it("renders one merged sizing section, not a separate drill section", () => {
    // Sizing and drill are still ONE section; the peer section beside it is a
    // different question, not the old standalone drill returning.
    renderInstanceOpportunity("additive", true);
    expect(screen.queryByTestId("wm-drill-toggle-checks.net_revenue")).toBeNull();
    expect(screen.getByTestId("wm-opp-toggle-checks.net_revenue")).toBeTruthy();
  });

  it("does not fetch any chain until a row is expanded", () => {
    renderInstanceOpportunity("additive", true);
    // Collapsed: the eager (hideWhenEmpty) opportunity scan runs; no drill does.
    expect(useDrillQuery).not.toHaveBeenCalled();

    // Mounted rows are the real question — a collapsed section trivially fetches
    // nothing. Open the section and its top dimension so the rows exist, and
    // assert they still cost zero drill queries while their own toggles are shut.
    fireEvent.click(screen.getByTestId("wm-opp-toggle-checks.net_revenue"));
    fireEvent.click(screen.getByTestId("wm-opp-dim-checks.channel"));
    expect(screen.getByTestId("wm-opp-drill-toggle-mobile_app")).toBeDefined();
    expect(useDrillQuery).not.toHaveBeenCalled();

    // …and only then does expanding a row issue one.
    fireEvent.click(screen.getByTestId("wm-opp-drill-toggle-mobile_app"));
    expect(useDrillQuery).toHaveBeenCalled();
  });
});
