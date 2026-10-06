// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useMonitorNotify, useMonitorPreview, useMonitors } from "@/hooks/api/useMetricAnomalies";
import type { MonitorEntry, MonitorPreview } from "@/types/metricAnomalies";
import MonitorsTab from "./MonitorsTab";

vi.mock("@/hooks/api/useMetricAnomalies", () => ({
  useMonitors: vi.fn(),
  useMonitorCoverage: () => ({ data: [] }),
  useMonitorNotify: vi.fn(),
  useMetricAnomalies: () => ({ data: undefined }),
  useMonitorPreview: vi.fn()
}));

const MONITORS: MonitorEntry[] = [
  {
    measure: "sales_daily.net_sales",
    time_dimension: "sales_daily.business_date",
    granularity: "day",
    lookback_days: 90,
    seasonality: null,
    sensitivity: "high",
    label: "Net sales",
    filters: [{ member: "sales_daily.region", values: ["US"] }]
  }
];

type PreviewHook = ReturnType<typeof useMonitorPreview>;

function mount(preview: Partial<PreviewHook> = {}, monitors: MonitorEntry[] = MONITORS) {
  vi.mocked(useMonitors).mockReturnValue({
    data: monitors,
    isLoading: false,
    error: null
  } as unknown as ReturnType<typeof useMonitors>);
  vi.mocked(useMonitorNotify).mockReturnValue({
    data: null
  } as unknown as ReturnType<typeof useMonitorNotify>);
  const hook = {
    isIdle: true,
    isPending: false,
    data: undefined,
    error: null,
    mutate: vi.fn(),
    reset: vi.fn(),
    ...preview
  } as unknown as PreviewHook;
  vi.mocked(useMonitorPreview).mockReturnValue(hook);
  render(<MonitorsTab />);
  return hook;
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("MonitorsTab", () => {
  it("sets a monitor's name over its measure and never badges a setting in the error colour", () => {
    mount();
    const row = screen.getByText("Net sales").closest("tr") as HTMLElement;
    expect(row).toHaveTextContent("sales_daily.net_sales");
    expect(row).toHaveTextContent("sales_daily.business_date");
    const badge = within(row).getByText("high");
    // `bg-destructive`, not any mention of the word: every badge's base
    // classes name the colour for its invalid state.
    expect(badge.className).not.toMatch(/(^|\s)bg-destructive/);
    expect(screen.getByTestId("monitors-delivery-note")).toHaveTextContent("only in this inbox");
  });

  // The selector has to name the entry the way the server finds it, filters
  // included — two entries over one measure differ only by them.
  it("asks for a dry run of exactly the row's entry, and shows nothing until asked", () => {
    const hook = mount();
    expect(screen.queryByTestId("monitor-preview")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("monitor-preview-run"));

    expect(hook.mutate).toHaveBeenCalledWith({
      measure: "sales_daily.net_sales",
      time_dimension: "sales_daily.business_date",
      granularity: "day",
      dimension_key: "sales_daily.region=US",
      group_by: null
    });
  });

  // A total and the same measure split per store share everything but
  // `group_by`. Without it in the request, the split's button previews the
  // total and presents a one-segment answer as the split's.
  it("names a fanned-out entry by its group_by, so its twin is not previewed instead", () => {
    const total = { ...MONITORS[0], filters: undefined };
    const perStore = { ...total, label: "Net sales by store", group_by: "sales_daily.store" };
    const hook = mount({}, [total, perStore]);

    fireEvent.click(screen.getAllByTestId("monitor-preview-run")[1]);

    expect(hook.mutate).toHaveBeenCalledWith({
      measure: "sales_daily.net_sales",
      time_dimension: "sales_daily.business_date",
      granularity: "day",
      dimension_key: "",
      group_by: "sales_daily.store"
    });
  });

  it("puts the answer under the row, says nothing was written, and can be dismissed", () => {
    const data: MonitorPreview = {
      window_buckets: 7,
      segments_total: 1,
      segments: [
        {
          dimension_key: "sales_daily.region=US",
          state: "scored",
          measured_buckets: 90,
          required_buckets: 56,
          flagged: [
            {
              timestamp: "2026-10-04T00:00:00Z",
              observed: 1204,
              expected: 1571,
              lower: 1400,
              upper: 1700,
              residual: -367,
              z_score: -4,
              severity: "high"
            }
          ]
        }
      ]
    };
    const hook = mount({ isIdle: false, data });

    const panel = screen.getByTestId("monitor-preview");
    expect(screen.getByTestId("monitor-preview-headline")).toHaveTextContent(
      "A scan now would flag 1 of the 7 most recent days."
    );
    expect(panel).toHaveTextContent("2026-10-04");
    expect(panel).toHaveTextContent("-23.4%");
    expect(panel).toHaveTextContent("Nothing was written");

    fireEvent.click(within(panel).getByLabelText("Close preview"));
    expect(hook.reset).toHaveBeenCalled();
  });

  it("shows a failed request as a failure, not as an empty answer", () => {
    mount({ isIdle: false, error: new Error("the warehouse did not answer within 50s") });
    const panel = screen.getByTestId("monitor-preview");
    expect(panel).toHaveTextContent("the warehouse did not answer within 50s");
    expect(screen.queryByTestId("monitor-preview-headline")).not.toBeInTheDocument();
    expect(panel).not.toHaveTextContent("Nothing was written");
  });
});
