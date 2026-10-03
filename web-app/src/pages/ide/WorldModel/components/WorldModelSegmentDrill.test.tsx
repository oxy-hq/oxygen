// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DrillLevel, DrillResponse } from "@/types/metricTree";
import { WorldModelSegmentDrill } from "./WorldModelSegmentDrill";

// This component reaches the API only through useDrillQuery, so mocking it
// covers every request the component can make — no QueryClientProvider needed.
const useDrillQuery = vi.hoisted(() => vi.fn());

vi.mock("@/hooks/api/useMetricTree", () => ({
  useDrillQuery
}));

function mockDrill(data: DrillResponse | undefined) {
  useDrillQuery.mockReturnValue({ data, isPending: false, error: null });
}

/**
 * Same two-level SHAPE the deleted `WorldModelDrillSection.test.tsx` asserted in
 * "attributes each followed split its OWN cascaded gap and share" — level 0's
 * `root_share === 1.0` invariant, a cascaded `root_share` at level 1, and a
 * terminal `stop_reason` — so both files describe the same engine contract.
 *
 * The candidate labels differ from that fixture on purpose: this drill is
 * rooted AT a specific row (`orders.channel = mobile_app`, the row under
 * test), so its chain plausibly decomposes by product line rather than
 * re-deriving `store_region`/`channel` splits of the whole population. The
 * "add-on" wording here is what the "renders the chain for its own row" test
 * below asserts on.
 */
function chainFixture(): DrillResponse {
  const levels: DrillLevel[] = [
    {
      measure: "orders.revenue",
      segment_filter: [],
      gap: -12000,
      root_share: 1.0,
      candidates: [
        {
          kind: { Dimension: { dimension: "product_line", value: "add-on" } },
          concentration: 0.75,
          gap: -9000,
          gated: true
        },
        {
          kind: { Dimension: { dimension: "product_line", value: "core" } },
          concentration: 0.2,
          gap: -2400,
          gated: true
        }
      ],
      stop_reason: null
    },
    {
      measure: "orders.revenue",
      segment_filter: [{ member: "orders.product_line", values: ["add-on"] }],
      gap: -9000,
      root_share: 0.75,
      candidates: [
        {
          kind: { Dimension: { dimension: "product_line", value: "warranty" } },
          concentration: 0.66,
          gap: -6000,
          gated: true
        }
      ],
      stop_reason: "GateFailed"
    }
  ];
  return {
    target: "orders.revenue",
    root_gap: -12000,
    root_upside: 12000,
    benchmark_filter: [],
    levels
  };
}

function renderSegmentDrill(overrides: {
  root?: { dimension: string; segment: string };
  instance?: { entity: string; key: string };
}) {
  return render(
    <WorldModelSegmentDrill
      nodeId='orders.revenue'
      timeDimension='orders.created_at'
      periodDays={90}
      root={overrides.root}
      instance={overrides.instance}
    />
  );
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WorldModelSegmentDrill", () => {
  it("sends the row's dimension and segment as the drill root", () => {
    mockDrill({ levels: [], root_gap: 0, root_upside: 0 });
    renderSegmentDrill({ root: { dimension: "orders.channel", segment: "mobile_app" } });
    expect(useDrillQuery).toHaveBeenCalledWith(
      expect.objectContaining({
        target: "orders.revenue",
        root: { dimension: "orders.channel", segment: "mobile_app" }
      }),
      true
    );
  });

  it("renders the chain for its own row", () => {
    // Two levels, so the followed split and the stop row both render.
    mockDrill(chainFixture());
    const { getByText } = renderSegmentDrill({
      root: { dimension: "orders.channel", segment: "mobile_app" }
    });
    expect(getByText(/add-on/)).toBeTruthy();
  });

  it("attributes each followed split its OWN cascaded gap and share, then the stop", () => {
    // Ported from the deleted WorldModelDrillSection.test.tsx — the engine
    // contract it pinned still holds, it just reaches the chain through this
    // component now. Real engine shape: levels[N].gap / root_share describe the
    // segment ENTERING level N, so levels[0].root_share === 1.0. The split
    // CHOSEN at level N is levels[N].candidates[0], and its own cascaded share
    // and magnitude are read from levels[N+1] — so the level-0 followed split
    // must render levels[1].root_share (0.75), NOT levels[0].root_share (1.0),
    // and levels[1].gap (-9000) as its magnitude.
    mockDrill(chainFixture());
    renderSegmentDrill({ root: { dimension: "orders.channel", segment: "mobile_app" } });

    // The headline counts followed splits, not the terminal stop level.
    expect(screen.getByText(/followed down 1 split/)).toBeDefined();
    expect(screen.getByText("product_line = add-on")).toBeDefined();
    expect(screen.getByText("+75% of root gap")).toBeDefined();
    expect(screen.queryByText("+100% of root gap")).toBeNull();
    expect(screen.getByText("-9.0k")).toBeDefined();
    // The last level renders as the stop only, in words — not a bare enum.
    expect(screen.getByText(/within sampling noise/)).toBeDefined();
    expect(screen.queryByText("GateFailed")).toBeNull();
    // The stopped level's candidate was CONSIDERED, not followed — it must not
    // render as an accepted chain step (it lives under a collapsed affordance).
    expect(screen.queryByText("product_line = warranty")).toBeNull();
    expect(screen.getByText(/1 split considered, not followed/)).toBeDefined();
    // Level 0's runner-up split is shown collapsed as a choice.
    expect(screen.getByText(/1 other split considered/)).toBeDefined();
  });

  it("marks an ungated FOLLOWED split unproven rather than claiming it", () => {
    // Also ported from the deleted section test. The drill still follows the
    // largest candidate even when its gap can't be told from noise — but the
    // followed split must be flagged unproven, never rendered as settled.
    const fixture = chainFixture();
    fixture.levels = fixture.levels?.map((l) => ({
      ...l,
      candidates: l.candidates.map((c) => ({ ...c, gated: false })),
      stop_reason: l.stop_reason ? "GateInconclusive" : null
    })) as DrillLevel[];
    mockDrill(fixture);

    renderSegmentDrill({ root: { dimension: "orders.channel", segment: "mobile_app" } });

    expect(screen.getByText("product_line = add-on")).toBeDefined();
    // The unproven caveat is present on the followed split…
    expect(screen.getByText("unproven")).toBeDefined();
    // …and the stop reason says the next split couldn't be proven.
    expect(screen.getByText(/couldn't be proven/)).toBeDefined();
  });

  it("shows the not-available state when the row is no longer in the scan", () => {
    // Engine returns Ok(None) -> no `levels` key. Must NOT render an empty
    // chain, which would read as "nothing wrong here" rather than "couldn't
    // decompose".
    mockDrill({});
    const { getByText } = renderSegmentDrill({
      root: { dimension: "orders.channel", segment: "gone" }
    });
    expect(getByText(/this row is no longer in the current scan/)).toBeTruthy();
  });

  it("scopes the drill to the instance when one is given", () => {
    mockDrill({ levels: [], root_gap: 0, root_upside: 0 });
    renderSegmentDrill({
      root: { dimension: "orders.channel", segment: "mobile_app" },
      instance: { entity: "store", key: "S1" }
    });
    expect(useDrillQuery).toHaveBeenCalledWith(
      expect.objectContaining({ instance: { entity: "store", key: "S1" } }),
      true
    );
  });
});
