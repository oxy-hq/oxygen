// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type DesignFormValues, emptyDesignValues } from "../designForm";
import MeasureFilterList from "./MeasureFilterList";

vi.mock("@/hooks/api/useMeasureFilterOptions", () => ({
  useMeasureDimensions: (measure: string) => ({
    view: measure.split(".")[0],
    dimensions: [
      { name: "sku", type: "string", description: "SKU" },
      { name: "category", type: "string" }
    ],
    isLoading: false
  }),
  useDimensionValues: (dimension: string) => ({
    data: dimension === "market_sku_days.sku" ? ["TB-30", "TB-40", "DP-20"] : [],
    isLoading: false,
    isError: false
  })
}));

// cmdk scrolls the active value into view, which jsdom does not implement.
Element.prototype.scrollIntoView ??= () => {};

afterEach(cleanup);

let latest: DesignFormValues | undefined;

function Harness() {
  const form = useForm<DesignFormValues>({ defaultValues: emptyDesignValues() });
  latest = form.watch();
  return (
    <MeasureFilterList
      form={form}
      messageFor={() => undefined}
      name='targetFilter'
      measure='market_sku_days.revenue'
      title='Only rows where'
    />
  );
}

describe("MeasureFilterList", () => {
  it("offers the measure's own dimensions and keeps the values picked from the data", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByTestId("experiment-design-target-filter-add"));

    const dimension = screen.getByTestId("experiment-design-target-filter-0-dimension");
    await user.click(dimension);
    const list = await screen.findByRole("listbox", { name: "Dimensions" });
    const options = Array.from(list.querySelectorAll("[role=option]")).map((o) =>
      o.getAttribute("data-value")
    );
    expect(options).toEqual(["market_sku_days.sku", "market_sku_days.category"]);
    await user.click(screen.getByRole("option", { name: /^sku/ }));

    await user.click(screen.getByTestId("experiment-design-target-filter-0-values"));
    await user.click(await screen.findByRole("option", { name: "TB-40" }));

    expect(latest?.targetFilter).toEqual([
      { dimension: "market_sku_days.sku", op: "equals", values: ["TB-40"] }
    ]);
  });
});
