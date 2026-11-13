// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import type { PreaggRollupStatus, PreaggStatusResponse } from "@/services/api/semantic";
import FieldsSelectionPanel from "./FieldsSelectionPanel";

const preagg = vi.hoisted(() => ({
  value: { data: undefined as PreaggStatusResponse | undefined }
}));
vi.mock("@/hooks/api/usePreaggStatus", () => ({ default: () => preagg.value }));

vi.mock("./contexts/ViewExplorerContext", () => ({
  useViewExplorerContext: () => ({
    viewData: {
      name: "orders",
      description: "",
      datasource: "warehouse",
      table: "public.orders",
      dimensions: [{ name: "order_status", type: "string" }],
      measures: [{ name: "total_orders", induced: false, promoted_from: null }]
    },
    selectedDimensions: [],
    setSelectedDimensions: vi.fn(),
    selectedMeasures: [],
    setSelectedMeasures: vi.fn(),
    toggleDimension: vi.fn(),
    toggleMeasure: vi.fn(),
    timeDimensions: [],
    onAddTimeDimension: vi.fn(),
    onUpdateTimeDimension: vi.fn(),
    onRemoveTimeDimension: vi.fn()
  })
}));

/** Built on some node in the fleet, but the Parquet is not on this one. */
const builtElsewhere: PreaggRollupStatus = {
  view_name: "orders",
  rollup_name: "orders_by_month",
  is_built: true,
  has_parquet: false,
  dimensions: ["order_status"],
  measures: [{ name: "total_orders", measure_type: "count" }],
  time_dimension: "order_date",
  granularity: "month",
  refresh_key: "every 1h",
  build_date: null,
  refresh_key_checked_at: "2026-05-11T14:03:22+00:00",
  empty_since: null
};

const serve = (blob_reads_available: boolean) => {
  preagg.value = { data: { rollups: [builtElsewhere], blob_reads_available } };
};

/** Renders the panel and expands the (collapsed-by-default) rollup section. */
const renderPanel = async () => {
  render(
    <SidebarProvider>
      <FieldsSelectionPanel />
    </SidebarProvider>
  );
  await userEvent.setup().click(screen.getByText(/Pre-aggregations/));
};

describe("FieldsSelectionPanel pre-aggregation state", () => {
  afterEach(cleanup);

  // The parity invariant `CacheState`/`CacheIcon` were extracted to hold. The
  // sidebar and the Pre-aggregation tab render the same rollup from the same
  // `usePreaggStatus()` payload, so they must not reach opposite conclusions
  // about whether queries still skip the warehouse. They did, once: the shared
  // component defaulted `blobReads` to false and this surface never passed it.
  it("says a rollup built elsewhere still skips the warehouse when blob reads work", async () => {
    serve(true);
    await renderPanel();
    expect(
      screen.getByTitle(/Built on another node\. Queries still skip the warehouse/)
    ).toBeInTheDocument();
  });

  it("says the warehouse answers when the deployment has no shared storage", async () => {
    serve(false);
    await renderPanel();
    expect(
      screen.getByTitle(/no shared storage configured, so queries here go/)
    ).toBeInTheDocument();
  });
});
