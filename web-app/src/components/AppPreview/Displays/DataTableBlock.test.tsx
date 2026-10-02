// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import {
  DateDay,
  Decimal,
  makeData,
  makeVector,
  Table,
  TimestampMillisecond,
  vectorFromArray
} from "apache-arrow";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TableDisplay } from "@/types/app";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));

// DuckDB answers `select * from "<file>"` with whatever table the test hands it.
const queryResult = vi.hoisted(() => ({ table: null as unknown }));
vi.mock("@/libs/duckdb", () => ({
  getDuckDB: () =>
    Promise.resolve({
      registerFileBuffer: () => Promise.resolve(),
      connect: () =>
        Promise.resolve({
          query: () => Promise.resolve(queryResult.table),
          close: () => Promise.resolve()
        })
    })
}));
vi.mock("@/services/api/axios", () => ({
  apiClient: { get: () => Promise.resolve({ data: new ArrayBuffer(0) }) }
}));

import { DataTableBlock } from "./DataTableBlock";

/**
 * A DECIMAL(18, scale) column as DuckDB returns it: each cell is the 128-bit
 * unscaled integer, and the scale is only on the column's type.
 */
const decimalColumn = (unscaled: bigint[], scale: number) => {
  const words = new BigInt64Array(unscaled.length * 2);
  unscaled.forEach((value, i) => {
    words[i * 2] = value;
    words[i * 2 + 1] = value < 0n ? -1n : 0n;
  });
  return makeVector(
    makeData({
      type: new Decimal(scale, 18, 128),
      length: unscaled.length,
      data: new Uint32Array(words.buffer)
    })
  );
};

const renderTable = async (table: Table, display: Partial<TableDisplay> = {}) => {
  queryResult.table = table;
  render(
    <DataTableBlock
      display={{ type: "table", data: "result.parquet", title: "Sales", ...display }}
      data={{}}
    />
  );
  await screen.findByRole("table");
  return screen.getAllByRole("cell").map((cell) => cell.textContent);
};

afterEach(cleanup);

describe("DataTableBlock decimal columns", () => {
  it("formats a decimal as currency when the column name implies money", async () => {
    // 1234.56 and -0.05, stored unscaled as 123456 and -5.
    const cells = await renderTable(new Table({ total_sales: decimalColumn([123456n, -5n], 2) }));
    expect(cells).toEqual(["$1,234.56", "-$0.05"]);
  });

  it("applies an explicit format to a decimal", async () => {
    const cells = await renderTable(new Table({ share: decimalColumn([1250n], 2) }), {
      formats: { share: "percent" }
    });
    expect(cells).toEqual(["12.5%"]);
  });

  it("formats a decimal that has no fractional digits", async () => {
    const cells = await renderTable(new Table({ revenue: decimalColumn([1234567n], 0) }));
    expect(cells).toEqual(["$1,234,567.00"]);
  });

  it("still scales a decimal that has no format", async () => {
    const cells = await renderTable(new Table({ quantity: decimalColumn([123456n], 2) }));
    expect(cells).toEqual(["1234.56"]);
  });
});

describe("DataTableBlock inferred currency", () => {
  it("is inferred from the name of a numeric column only", async () => {
    const cells = await renderTable(
      new Table({
        payment_date: vectorFromArray([new Date("2024-03-05T00:00:00Z")], new DateDay()),
        payment_at: vectorFromArray([new Date("2024-03-05T12:34:56Z")], new TimestampMillisecond()),
        discount_code: vectorFromArray(["2024"]),
        unit_price: makeVector(new Float64Array([12.5])),
        order_revenue: makeVector(new BigInt64Array([1234n])),
        refund_fee: makeVector(new Int32Array([7]))
      })
    );
    expect(cells).toEqual([
      "2024-03-05",
      "2024-03-05 12:34",
      "2024",
      "$12.50",
      "$1,234.00",
      "$7.00"
    ]);
  });

  it("keeps an explicit format on a numeric column, whatever its name", async () => {
    const cells = await renderTable(
      new Table({
        orders: makeVector(new Int32Array([1234567])),
        unit_price: makeVector(new Float64Array([12.5]))
      }),
      { formats: { orders: "number", unit_price: "percent" } }
    );
    expect(cells).toEqual(["1,234,567", "12.5%"]);
  });
});
