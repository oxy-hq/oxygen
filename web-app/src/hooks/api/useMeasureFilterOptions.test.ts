// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { distinctValues } from "./useMeasureFilterOptions";

describe("distinctValues", () => {
  it("drops the header and keeps each value once, in order", () => {
    // Regression: a dimension-only query returned raw rows, so TB-40 was offered twice.
    const rows = [
      ["market_sku_days__sku", "market_sku_days__revenue"],
      ["TB-40", "10"],
      ["TB-30", "9"],
      ["TB-40", "8"],
      ["", "1"]
    ];
    expect(distinctValues(rows)).toEqual(["TB-40", "TB-30"]);
  });

  it("reads anything that is not rows as no values", () => {
    expect(distinctValues({ file_name: "x.parquet" })).toEqual([]);
  });
});
