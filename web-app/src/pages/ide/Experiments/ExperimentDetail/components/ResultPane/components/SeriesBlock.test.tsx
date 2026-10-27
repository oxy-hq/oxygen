// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DailySeries } from "@/types/experiment";
import SeriesBlock from "./SeriesBlock";

vi.mock("@/components/Echarts", () => ({
  Echarts: ({ testId }: { testId?: string }) => <div data-testid={testId} />
}));
vi.mock("@/components/Echarts/resolveColor", () => ({
  resolveColor: () => "#000",
  resolveColorWithAlpha: () => "rgba(0,0,0,0.1)"
}));

afterEach(cleanup);

const series: DailySeries = {
  measure: "store_days.revenue",
  measured_until: null,
  design: "waves",
  treated_units: 2,
  control_units: 1,
  waves: [
    {
      switch_on: "2026-03-05",
      units: 2,
      pre_from: "2026-03-02",
      pre_to: "2026-03-04",
      post_from: "2026-03-06",
      post_to: "2026-03-08"
    }
  ],
  days: [
    { date: "2026-03-02", treated: 3, control: 10, on_units: 0 },
    { date: "2026-03-06", treated: 7.5, control: null, on_units: 2 }
  ]
};

describe("SeriesBlock", () => {
  it("shows the summary, the chart and one table row per day", () => {
    render(<SeriesBlock series={series} />);
    expect(screen.getByTestId("experiment-report-series")).toHaveTextContent(
      "2 treated and 1 control units, 2026-03-02 to 2026-03-06 (2 days)"
    );
    expect(screen.getByTestId("experiment-report-series-chart")).toBeInTheDocument();
    const rows = within(screen.getByTestId("experiment-report-series-table")).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveTextContent("2026-03-02");
    expect(rows[1]).toHaveTextContent("3.00");
    expect(rows[1]).toHaveTextContent("-7.00");
    expect(rows[1]).toHaveTextContent("Pre window");
    // A missing control mean is a dash, never 0, and so is the difference.
    expect(rows[2]).toHaveTextContent("7.50");
    expect(rows[2]).toHaveTextContent("—");
    expect(rows[2]).not.toHaveTextContent("0.00");
  });

  it("puts the fleet mean in a switchback's table", () => {
    render(
      <SeriesBlock
        series={{
          measure: "m",
          measured_until: null,
          design: "switchback",
          fleet_units: 4,
          washout_days: 1,
          periods: [{ from: "2026-04-01", to: "2026-04-08", on: true }],
          days: [{ date: "2026-04-01", fleet: 12.5, on: true, washout: true }]
        }}
      />
    );
    const table = screen.getByTestId("experiment-report-series-table");
    expect(within(table).getByRole("columnheader", { name: "Fleet mean" })).toBeInTheDocument();
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("12.50");
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("Washout");
  });
});
