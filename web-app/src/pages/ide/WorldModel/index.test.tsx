// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorldModelService } from "@/services/api/worldModel";
import type { WmComputedMeasure } from "@/types/worldModel";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));
vi.mock("@/services/api/worldModel", () => ({
  WorldModelService: {
    getWorldModel: vi.fn(),
    streamFilterCounts: vi.fn(),
    streamInstanceDetail: vi.fn()
  }
}));
// Not under test: the canvas, the panel and the popovers. The graph stands in as
// the one thing the page needs from it here — picking an instance to filter by —
// and shows the measure values the page hands its cards.
vi.mock("../components/semanticGraph", () => ({ PANEL_WIDTH: "w-80" }));
vi.mock("./components/WorldModelGraph", () => ({
  WorldModelGraph: ({
    seedComputedMeasures,
    onSelectChildInstance
  }: {
    seedComputedMeasures: WmComputedMeasure[] | null;
    onSelectChildInstance: (entityId: string, key: string, display: string) => void;
  }) => (
    <div>
      <button type='button' onClick={() => onSelectChildInstance("store", "42", "Store 42")}>
        Filter by Store 42
      </button>
      <output>{(seedComputedMeasures ?? []).map((m) => `${m.name}=${m.value}`).join(",")}</output>
    </div>
  )
}));
vi.mock("./components/WorldModelDetailPanel", () => ({ WorldModelDetailPanel: () => null }));
vi.mock("./components/InstancePickerPopover", () => ({ InstancePickerPopover: () => null }));
vi.mock("./components/SampleBrowserPopover", () => ({ SampleBrowserPopover: () => null }));

import WorldModelView from "./index";

const getWorldModel = vi.mocked(WorldModelService.getWorldModel);
const streamFilterCounts = vi.mocked(WorldModelService.streamFilterCounts);
const streamInstanceDetail = vi.mocked(WorldModelService.streamInstanceDetail);

/** Opens the page and filters it by one store. */
const filterByStore = async () => {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <WorldModelView />
    </QueryClientProvider>
  );
  fireEvent.click(await screen.findByRole("button", { name: "Filter by Store 42" }));
};

beforeEach(() => {
  getWorldModel.mockResolvedValue({ entities: [], edges: [] });
  // The counts arrive without trouble: only the instance's own stream varies.
  streamFilterCounts.mockImplementation((_p, _e, _k, onEvent, onClose) => {
    queueMicrotask(() => {
      onEvent({ entity_name: "store", done: true });
      onClose();
    });
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WorldModelView filter seed", () => {
  it("says so on the filter pill when the seed's measures fail to load", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, _onEvent, _onClose, onError) => {
      queueMicrotask(() => onError(new Error("SSE connection failed with status: 500")));
    });

    await filterByStore();

    expect(await screen.findByText("Failed to load measures")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("");
  });

  it("hands the cards the seed's measure values, with nothing to report", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent({
          kind: "measure_names",
          measure_names: [{ name: "revenue", measure_type: "sum" }]
        });
        onEvent({
          kind: "init",
          entity_id: "store",
          key_value: "42",
          display: "Store 42",
          attributes: []
        });
        onEvent({
          kind: "measure",
          computed_measures: [{ name: "revenue", measure_type: "sum", value: "10", fiber_count: 3 }]
        });
        onEvent({ kind: "done" });
        onClose();
      });
    });

    await filterByStore();

    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("revenue=10"));
    expect(screen.queryByText(/Failed to load/)).toBeNull();
  });
});
