// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import {
  DateDay,
  Decimal,
  Float64,
  Int32,
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

// DuckDB answers `select * from "<file>"` with whatever table the test hands it,
// or fails the query with `error` once a test sets one.
const queryResult = vi.hoisted(() => ({ table: null as unknown, error: null as Error | null }));
vi.mock("@/libs/duckdb", () => ({
  getDuckDB: () =>
    Promise.resolve({
      registerFileBuffer: () => Promise.resolve(),
      connect: () =>
        Promise.resolve({
          query: () =>
            queryResult.error
              ? Promise.reject(queryResult.error)
              : Promise.resolve(queryResult.table),
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

afterEach(() => {
  cleanup();
  queryResult.error = null;
  vi.restoreAllMocks();
});

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

  it("is not inferred for a number that is not an amount of money", async () => {
    const cells = await renderTable(
      new Table({
        payment_id: makeVector(new Int32Array([1234])),
        discount_count: makeVector(new BigInt64Array([1234n])),
        revenue_pct: makeVector(new Float64Array([12.5])),
        payment_year: makeVector(new Int32Array([2024])),
        payment_amount: makeVector(new Int32Array([1234]))
      })
    );
    expect(cells).toEqual(["1234", "1234", "12.50", "2024", "$1,234.00"]);
  });

  it("keeps an explicit format on a column the name rule would not infer", async () => {
    const cells = await renderTable(
      new Table({ discount_count: makeVector(new Int32Array([1234])) }),
      { formats: { discount_count: "currency" } }
    );
    expect(cells).toEqual(["$1,234.00"]);
  });
});

describe("DataTableBlock NULL cells", () => {
  it("are empty in every column, formatted or not", async () => {
    const cells = await renderTable(
      new Table({
        label: vectorFromArray([null, "north"]),
        quantity: vectorFromArray([null, 3], new Int32()),
        total_sales: vectorFromArray([null, 12.5], new Float64())
      })
    );
    expect(cells).toEqual(["", "", "", "north", "3", "$12.50"]);
  });
});

describe("DataTableBlock load failure", () => {
  it("shows the failure, not an empty result", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    queryResult.error = new Error("Out of memory");

    render(
      <DataTableBlock
        display={{ type: "table", data: "result.parquet", title: "Sales" }}
        data={{}}
      />
    );

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Failed to load this table");
    expect(alert.textContent).toContain("Out of memory");
    expect(screen.queryByText("No data found")).toBeNull();
  });

  it("still says an empty result has no data", async () => {
    render(
      <DataTableBlock
        display={{ type: "table", data: "orders", title: "Sales" }}
        data={{ orders: { file_path: "orders.parquet", json: "[]" } }}
      />
    );

    expect(await screen.findByText("No data found")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
