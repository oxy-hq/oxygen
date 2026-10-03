import { createRequire } from "node:module";
import * as ownArrow from "apache-arrow";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/libs/duckdb", () => ({ getDuckDB: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));

import { getDuckDB } from "@/libs/duckdb";
import {
  cellText,
  formatValue,
  getArrowColumnKeys,
  getArrowColumnValues,
  getArrowExportText,
  getArrowResultCell,
  inferColumnFormat,
  isNumericType,
  runSqlInDuckDB
} from "./utils";

// Query results are built by the Arrow release duckdb-wasm depends on, which is
// older than the one this app imports. Cells must read the same from either.
const require = createRequire(import.meta.url);
const duckdbArrow = createRequire(require.resolve("@duckdb/duckdb-wasm"))(
  "apache-arrow"
) as typeof ownArrow;

describe.each([
  ["the app's Arrow", ownArrow],
  ["duckdb-wasm's Arrow", duckdbArrow]
])("Arrow cells from %s", (_name, arrow) => {
  /** One row of a table, with each column's type, as a query result has them. */
  const firstRow = (columns: Record<string, ownArrow.Vector>) => {
    const table = new arrow.Table(columns);
    const row = table.toArray()[0] as Record<string, unknown>;
    const typeOf = (name: string) => table.schema.fields.find((f) => f.name === name)?.type;
    return { row, typeOf };
  };

  /** A DECIMAL(38, scale) column holding these unscaled integers. */
  const decimals = (unscaled: bigint[], scale: number) => {
    const words = new BigInt64Array(unscaled.length * 2);
    unscaled.forEach((value, i) => {
      words[i * 2] = BigInt.asIntN(64, value);
      words[i * 2 + 1] = value >> 64n;
    });
    return arrow.makeVector(
      arrow.makeData({
        type: new arrow.Decimal(scale, 38, 128),
        length: unscaled.length,
        data: new Uint32Array(words.buffer)
      })
    );
  };

  describe("formatValue", () => {
    it("reads a decimal cell as the number it stands for", () => {
      const { row, typeOf } = firstRow({ amount: decimals([123456n], 2) });
      const type = typeOf("amount");

      // The cell alone is the unscaled integer: this is what used to be shown.
      expect(cellText(row.amount)).toBe("123456");

      expect(formatValue(row.amount, "currency", { type })).toBe("$1,234.56");
      expect(formatValue(row.amount, "number", { type })).toBe("1,234.56");
      expect(formatValue(row.amount, "percent", { type })).toBe("1,234.56%");
      expect(formatValue(row.amount, "currency", { type, compact: true })).toBe("$1.2K");
      expect(formatValue(row.amount, undefined, { type })).toBe("1234.56");
    });

    it("keeps the sign and the leading zeros of a small decimal", () => {
      const { row, typeOf } = firstRow({ amount: decimals([-5n], 2) });
      expect(formatValue(row.amount, "currency", { type: typeOf("amount") })).toBe("-$0.05");
    });

    it("formats a scale-0 decimal", () => {
      const { row, typeOf } = firstRow({ amount: decimals([1234567n], 0) });
      expect(formatValue(row.amount, "currency", { type: typeOf("amount") })).toBe("$1,234,567.00");
    });

    it("formats a decimal too large for a safe integer instead of throwing", () => {
      // Arrow's own number conversion throws past 2^53; this one settles for
      // the nearest double, so the cents of a quintillion are lost.
      const { row, typeOf } = firstRow({ amount: decimals([10n ** 20n + 25n], 2) });
      const type = typeOf("amount");
      expect(formatValue(row.amount, "number", { type })).toBe("1,000,000,000,000,000,000");
      // An export keeps them.
      expect(getArrowExportText(row.amount, type)).toBe("1000000000000000000.25");
    });

    it("leaves non-decimal values as they were, with or without a type", () => {
      const { row, typeOf } = firstRow({
        price: arrow.makeVector(new Float64Array([12.5])),
        label: arrow.vectorFromArray(["north"])
      });
      expect(formatValue(row.price, "currency", { type: typeOf("price") })).toBe("$12.50");
      expect(formatValue(row.price, "currency")).toBe("$12.50");
      expect(formatValue(row.label, "currency", { type: typeOf("label") })).toBe("north");
      expect(formatValue(10n, "number")).toBe("10");
      expect(formatValue("42.5", "percent")).toBe("42.5%");
      expect(formatValue(null, "currency")).toBe("");
    });

    it("prints a number with no format as the number it is, not rounded to two places", () => {
      // A pie slice's tooltip, when the value column has no format.
      expect(formatValue(0.004)).toBe("0.004");
      expect(formatValue("0.123456")).toBe("0.123456");
      expect(formatValue(1234)).toBe("1234");
      // A declared format still rounds as it says.
      expect(formatValue(0.004, "number")).toBe("0");
    });
  });

  describe("a chart's column", () => {
    it("reads a number as the value the column holds, for a chart to plot", () => {
      const table = new arrow.Table({
        tiny: arrow.makeVector(new Float64Array([0.004])),
        ratio: arrow.makeVector(new Float64Array([0.123456])),
        tenth: arrow.makeVector(new Float32Array([0.1])),
        rate: decimals([1234567n], 6),
        small: decimals([4n], 3),
        count: arrow.makeVector(new Int32Array([42]))
      });
      const value = (column: string) => getArrowColumnValues(table, column)[0];
      // ECharts reads each as the number it spells. 0.004 used to be "0.00",
      // so the chart drew it as 0; a decimal of 0.004 the same.
      expect(value("tiny")).toBe("0.004");
      expect(value("ratio")).toBe("0.123456");
      expect(value("tenth")).toBe("0.1");
      expect(value("rate")).toBe("1.234567");
      expect(value("small")).toBe("0.004");
      expect(value("count")).toBe("42");
    });
  });

  describe("a NULL cell", () => {
    it("passes through getArrowResultCell whatever the column type", () => {
      const { row, typeOf } = firstRow({
        amount: arrow.makeVector(
          arrow.makeData({
            type: new arrow.Decimal(2, 38, 128),
            length: 1,
            nullCount: 1,
            nullBitmap: new Uint8Array([0]),
            data: new Uint32Array(4)
          })
        ),
        day: arrow.vectorFromArray([null], new arrow.DateDay()),
        at: arrow.vectorFromArray([null], new arrow.TimestampMillisecond()),
        time: arrow.vectorFromArray([null], new arrow.TimeMicrosecond()),
        label: arrow.vectorFromArray([null], new arrow.Utf8())
      });
      // A NULL text cell already came through as null: the table shows it empty.
      expect(row.label).toBeNull();
      expect(getArrowResultCell(row.label, typeOf("label") as ownArrow.DataType)).toBeNull();

      // A NULL decimal used to throw, and a NULL date or time read "Invalid Date".
      for (const column of ["amount", "day", "at", "time"]) {
        expect(row[column]).toBeNull();
        expect(getArrowResultCell(row[column], typeOf(column) as ownArrow.DataType)).toBeNull();
      }
    });
  });

  describe("isNumericType", () => {
    it("is true of number columns and of nothing else", () => {
      const { typeOf } = firstRow({
        int: arrow.makeVector(new Int32Array([1])),
        bigint: arrow.makeVector(new BigInt64Array([1n])),
        float: arrow.makeVector(new Float64Array([1.5])),
        decimal: decimals([150n], 2),
        day: arrow.vectorFromArray([new Date(0)], new arrow.DateDay()),
        at: arrow.vectorFromArray([new Date(0)], new arrow.TimestampMillisecond()),
        label: arrow.vectorFromArray(["2024"]),
        flag: arrow.vectorFromArray([true])
      });
      const numeric = ["int", "bigint", "float", "decimal", "day", "at", "label", "flag"].filter(
        (column) => isNumericType(typeOf(column))
      );
      expect(numeric).toEqual(["int", "bigint", "float", "decimal"]);
      expect(isNumericType(undefined)).toBe(false);
    });
  });

  describe("inferColumnFormat", () => {
    const float = new arrow.Float64();

    it.each([
      "total_sales",
      "oxymart__total_weekly_sales",
      "unit_price",
      "order_revenue",
      "refund_fee",
      "avg_cost",
      "price_usd",
      "Total Revenue",
      // A calendar word after a period qualifier is the period the money is for.
      "revenue_last_month",
      "sales_per_day",
      "prior_year_revenue",
      // camelCase and PascalCase names are split into words like snake_case ones.
      "totalRevenue",
      "TotalSales",
      "avgOrderValueUSD",
      // An amount of a margin or a discount is money.
      "margin_amount",
      "discount_usd",
      "discountedPrice",
      // A count of orders is no money; money from orders is.
      "orders_revenue"
    ])("reads %s as an amount of money", (name) => {
      expect(inferColumnFormat(name, float)).toBe("currency");
    });

    it.each([
      // identifiers and codes
      "payment_id",
      "price_key",
      "discount_code",
      "payment_number",
      // flags and enumerations
      "discount_flag",
      "is_discount",
      "payment_status",
      "payment_type",
      // tallies and quantities
      "discount_count",
      "num_payments",
      "sales_qty",
      "sales_units",
      // ratios
      "revenue_pct",
      "discount_rate",
      "revenue_share",
      "sales_growth",
      // positions on a scale
      "price_rank",
      "price_index",
      // calendar parts
      "payment_year",
      "sales_month",
      // a margin or a discount, which is usually a ratio: 0.25 is not $0.25
      "profit_margin",
      "gross_margin",
      "margin",
      "total_discount",
      "avgDiscount",
      // a count of the things a monetary word describes
      "sales_orders",
      "sales_reps",
      "payment_transactions",
      "salesCalls",
      // camelCase names are read by the same rules
      "paymentId",
      "discountCount",
      "revenuePct",
      // no monetary word at all
      "holiday_flag",
      "oxymart__store"
    ])("does not read %s as money", (name) => {
      expect(inferColumnFormat(name, float)).toBeUndefined();
    });

    it("infers for a numeric column of any kind, and for no other", () => {
      const { typeOf } = firstRow({
        int: arrow.makeVector(new Int32Array([1])),
        bigint: arrow.makeVector(new BigInt64Array([1n])),
        float: arrow.makeVector(new Float64Array([1.5])),
        decimal: decimals([150n], 2),
        day: arrow.vectorFromArray([new Date(0)], new arrow.DateDay()),
        label: arrow.vectorFromArray(["2024"]),
        flag: arrow.vectorFromArray([true])
      });
      const inferred = ["int", "bigint", "float", "decimal", "day", "label", "flag"].filter(
        (column) => inferColumnFormat("total_sales", typeOf(column)) === "currency"
      );
      expect(inferred).toEqual(["int", "bigint", "float", "decimal"]);
      expect(inferColumnFormat("total_sales", undefined)).toBeUndefined();
      expect(inferColumnFormat(undefined, float)).toBeUndefined();
    });
  });

  describe("getArrowResultCell", () => {
    const cellOf = (columns: Record<string, ownArrow.Vector>) => {
      const { row, typeOf } = firstRow(columns);
      return (column: string) =>
        getArrowResultCell(row[column], typeOf(column) as ownArrow.DataType);
    };

    it("shows a float as the value it holds, not rounded to two places", () => {
      const cell = cellOf({
        ratio: arrow.makeVector(new Float64Array([0.123456])),
        tiny: arrow.makeVector(new Float64Array([0.004])),
        half: arrow.makeVector(new Float64Array([1.5])),
        whole: arrow.makeVector(new Float64Array([3])),
        negative: arrow.makeVector(new Float64Array([-0.000125]))
      });
      expect(cell("ratio")).toBe("0.123456");
      expect(cell("tiny")).toBe("0.004");
      // No digit is added either: 1.5 is not "1.50".
      expect(cell("half")).toBe("1.5");
      expect(cell("whole")).toBe("3");
      expect(cell("negative")).toBe("-0.000125");
    });

    it("shows a single-precision float without the digits widening it to a double adds", () => {
      const cell = cellOf({
        tenth: arrow.makeVector(new Float32Array([0.1])),
        third: arrow.makeVector(new Float32Array([1 / 3])),
        whole: arrow.makeVector(new Float32Array([16777216]))
      });
      // The cell arrives as the double 0.10000000149011612.
      expect(cell("tenth")).toBe("0.1");
      expect(cell("third")).toBe("0.33333334");
      expect(cell("whole")).toBe("16777216");
    });

    it("shows a decimal at its declared scale, every digit", () => {
      const cell = cellOf({
        rate: decimals([1234567n], 6),
        amount: decimals([150n], 2),
        small: decimals([-5n], 2),
        // Past 2^53, where reading it through a double loses the cents.
        large: decimals([10n ** 20n + 25n], 2)
      });
      expect(cell("rate")).toBe("1.234567");
      expect(cell("amount")).toBe("1.50");
      expect(cell("small")).toBe("-0.05");
      expect(cell("large")).toBe("1000000000000000000.25");
    });

    it("shows a timestamp's seconds, and its milliseconds when it has any, as an export does", () => {
      const cell = cellOf({
        at: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56Z")],
          new arrow.TimestampMicrosecond()
        ),
        precise: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56.789Z")],
          new arrow.TimestampMillisecond()
        ),
        local: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56Z")],
          new arrow.TimestampMillisecond("America/New_York")
        ),
        snowflake: arrow.vectorFromArray(
          [{ epoch: 1709642096n, fraction: 789_000_000 }],
          new arrow.Struct([
            new arrow.Field("epoch", new arrow.Int64()),
            new arrow.Field("fraction", new arrow.Int32())
          ])
        )
      });
      // Each used to stop at the minute, "2024-03-05 12:34".
      expect(cell("at")).toBe("2024-03-05 12:34:56");
      expect(cell("precise")).toBe("2024-03-05 12:34:56.789");
      expect(cell("local")).toBe("2024-03-05 07:34:56");
      expect(cell("snowflake")).toBe("2024-03-05 12:34:56.789");
    });

    it("reads every other cell as before", () => {
      const cell = cellOf({
        count: arrow.makeVector(new Int32Array([42])),
        big: arrow.makeVector(new BigInt64Array([9007199254740993n])),
        day: arrow.vectorFromArray([new Date("2024-03-05T00:00:00Z")], new arrow.DateDay()),
        label: arrow.vectorFromArray(["north"]),
        missing: arrow.vectorFromArray([null], new arrow.Float64())
      });
      expect(cell("count")).toBe("42");
      expect(cell("big")).toBe("9007199254740993");
      expect(cell("day")).toBe("2024-03-05");
      expect(cell("label")).toBe("north");
      expect(cell("missing")).toBeNull();
    });
  });

  describe("a 128-bit integer: DuckDB's HUGEINT, and the SUM of an integer column", () => {
    // DuckDB hands both over as DECIMAL(38, 0): each cell is four 32-bit words.
    const above32Bits = 5_000_000_000n;
    const sums = () => decimals([above32Bits, -above32Bits, -(2n ** 100n) - 7n], 0);

    it("reads in a table, a chart and an export as every digit it has", () => {
      const table = new arrow.Table({ total: sums() });
      const type = table.schema.fields[0].type as ownArrow.DataType;
      const cells = (table.toArray() as Record<string, unknown>[]).map((row) => row.total);

      const whole = ["5000000000", "-5000000000", "-1267650600228229401496703205383"];
      expect(cells.map((cell) => getArrowResultCell(cell, type))).toEqual(whole);
      expect(getArrowColumnValues(table, "total")).toEqual(whole);
      expect(cells.map((cell) => getArrowExportText(cell, type))).toEqual(whole);
    });

    it("is written whole, and a decimal at its scale, when the app re-runs a task in the browser", async () => {
      const table = new arrow.Table({ total: sums(), amount: decimals([1234n, -5n, 0n], 2) });
      vi.mocked(getDuckDB).mockResolvedValue({
        connect: () =>
          Promise.resolve({ query: () => Promise.resolve(table), close: () => Promise.resolve() })
      } as unknown as Awaited<ReturnType<typeof getDuckDB>>);

      const rows = JSON.parse(await runSqlInDuckDB("select 1")) as Record<string, number>[];

      // The lowest word alone used to be written: 705032704 for 5000000000, and
      // a decimal's unscaled digits, 1234 for 12.34. A JSON number is a double,
      // so a value past 2^53 keeps a double's precision and no more.
      expect(rows).toEqual([
        { total: 5_000_000_000, amount: 12.34 },
        { total: -5_000_000_000, amount: -0.05 },
        { total: -(2 ** 100) - 7, amount: 0 }
      ]);
    });
  });

  describe("a task the app re-runs in the browser", () => {
    /** What `runSqlInDuckDB` writes when DuckDB answers with `table`. */
    const writtenFor = async (table: ownArrow.Table) => {
      vi.mocked(getDuckDB).mockResolvedValue({
        connect: () =>
          Promise.resolve({ query: () => Promise.resolve(table), close: () => Promise.resolve() })
      } as unknown as Awaited<ReturnType<typeof getDuckDB>>);
      return JSON.parse(await runSqlInDuckDB("select 1")) as Record<string, unknown>[];
    };

    it("writes a date or a time as the table shows it, so that it is read back as one", async () => {
      const rows = await writtenFor(
        new arrow.Table({
          day: arrow.vectorFromArray([new Date("2024-03-05T00:00:00Z")], new arrow.DateDay()),
          at: arrow.vectorFromArray(
            [new Date("2024-03-05T12:34:56.789Z")],
            new arrow.TimestampMicrosecond()
          ),
          whole: arrow.vectorFromArray(
            [new Date("2024-03-05T12:34:56Z")],
            new arrow.TimestampMillisecond()
          ),
          time: arrow.vectorFromArray([45_296_000], new arrow.TimeMillisecond())
        })
      );

      // Each used to be written as a number (1709596800000 for the day), which
      // `read_json_auto` reads back as a BIGINT: the column was a date no more.
      expect(rows).toEqual([
        {
          day: "2024-03-05",
          at: "2024-03-05 12:34:56.789",
          whole: "2024-03-05 12:34:56",
          time: "12:34:56"
        }
      ]);
    });

    it("writes a moment with a time zone as its instant, not as the time on a clock there", async () => {
      const rows = await writtenFor(
        new arrow.Table({
          utc: arrow.vectorFromArray(
            [new Date("2024-03-05T12:34:56.789Z")],
            new arrow.TimestampMicrosecond("UTC")
          ),
          local: arrow.vectorFromArray(
            [new Date("2024-03-05T12:34:56Z")],
            new arrow.TimestampMillisecond("America/New_York")
          )
        })
      );

      // The table shows the second as 07:34:56, which says nothing of New York.
      expect(rows).toEqual([
        { utc: "2024-03-05T12:34:56.789Z", local: "2024-03-05T12:34:56.000Z" }
      ]);
    });

    it("refuses a column it cannot write as it is, rather than write its first byte", async () => {
      const blobs = new arrow.Table({
        id: arrow.makeVector(new Int32Array([1])),
        payload: arrow.vectorFromArray([new Uint8Array([7, 8, 9])], new arrow.Binary())
      });

      // A BLOB was written as 7. The app runs the task on the server instead.
      await expect(writtenFor(blobs)).rejects.toThrow(/"payload"/);
    });
  });

  describe("a chart's column of moments", () => {
    const moments = (...at: (string | null)[]) =>
      new arrow.Table({
        at: arrow.vectorFromArray(
          at.map((value) => (value === null ? null : new Date(value))),
          new arrow.TimestampMillisecond()
        )
      });

    it("is labelled to the precision its values need, the same for every one", () => {
      expect(
        getArrowColumnValues(moments("2024-03-05T12:34:00Z", "2024-03-05T12:35:00Z"), "at")
      ).toEqual(["2024-03-05 12:34", "2024-03-05 12:35"]);
      expect(
        getArrowColumnValues(moments("2024-03-05T12:34:00Z", "2024-03-05T12:34:30Z", null), "at")
      ).toEqual(["2024-03-05 12:34:00", "2024-03-05 12:34:30", null]);
      expect(
        getArrowColumnValues(moments("2024-03-05T12:34:56Z", "2024-03-05T12:34:56.789Z"), "at")
      ).toEqual(["2024-03-05 12:34:56.000", "2024-03-05 12:34:56.789"]);
    });

    it("is labelled to the precision of a Snowflake timestamp's fraction", () => {
      const table = new arrow.Table({
        at: arrow.vectorFromArray(
          [
            { epoch: 1709642040n, fraction: 0 },
            { epoch: 1709642096n, fraction: 0 }
          ],
          new arrow.Struct([
            new arrow.Field("epoch", new arrow.Int64()),
            new arrow.Field("fraction", new arrow.Int32())
          ])
        )
      });
      expect(getArrowColumnValues(table, "at")).toEqual([
        "2024-03-05 12:34:00",
        "2024-03-05 12:34:56"
      ]);
    });

    it("is keyed by each moment's exact instant, whatever the others need", () => {
      // A series holding only whole minutes is matched to an axis that has seconds.
      expect(getArrowColumnKeys(moments("2024-03-05T12:34:00Z"), "at")).toEqual([
        "2024-03-05T12:34:00.000Z"
      ]);
      expect(
        getArrowColumnKeys(moments("2024-03-05T12:34:00Z", "2024-03-05T12:34:56.789Z"), "at")
      ).toEqual(["2024-03-05T12:34:00.000Z", "2024-03-05T12:34:56.789Z"]);
    });

    it("keys two moments apart that read the same on a clock in their zone", () => {
      // 01:30 in New York, before and after the clocks go back.
      const table = new arrow.Table({
        at: arrow.vectorFromArray(
          [new Date("2024-11-03T05:30:00Z"), new Date("2024-11-03T06:30:00Z")],
          new arrow.TimestampMillisecond("America/New_York")
        )
      });
      expect(getArrowColumnValues(table, "at")).toEqual(["2024-11-03 01:30", "2024-11-03 01:30"]);
      expect(getArrowColumnKeys(table, "at")).toEqual([
        "2024-11-03T05:30:00.000Z",
        "2024-11-03T06:30:00.000Z"
      ]);
    });
  });

  describe("getArrowExportText", () => {
    it("writes a decimal with its scale and every digit", () => {
      const { row, typeOf } = firstRow({ rate: decimals([1234567n, -5n], 6) });
      const type = typeOf("rate");
      expect(getArrowExportText(row.rate, type)).toBe("1.234567");
      // The result table and a chart read the same digits.
      expect(getArrowResultCell(row.rate, type as ownArrow.DataType)).toBe("1.234567");
    });

    it("pads a decimal smaller than one and keeps its sign", () => {
      const { row, typeOf } = firstRow({ rate: decimals([-5n], 2) });
      expect(getArrowExportText(row.rate, typeOf("rate"))).toBe("-0.05");
    });

    it("writes a date and a timestamp as dates, not epoch numbers", () => {
      const { row, typeOf } = firstRow({
        day: arrow.vectorFromArray([new Date("2024-03-05T00:00:00Z")], new arrow.DateDay()),
        at: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56Z")],
          new arrow.TimestampMicrosecond()
        ),
        precise: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56.789Z")],
          new arrow.TimestampMillisecond()
        ),
        local: arrow.vectorFromArray(
          [new Date("2024-03-05T12:34:56Z")],
          new arrow.TimestampMillisecond("America/New_York")
        )
      });
      // The cells themselves are epoch milliseconds: this is what used to be exported.
      expect(cellText(row.day)).toBe("1709596800000");

      expect(getArrowExportText(row.day, typeOf("day"))).toBe("2024-03-05");
      expect(getArrowExportText(row.at, typeOf("at"))).toBe("2024-03-05 12:34:56");
      expect(getArrowExportText(row.precise, typeOf("precise"))).toBe("2024-03-05 12:34:56.789");
      expect(getArrowExportText(row.local, typeOf("local"))).toBe("2024-03-05 07:34:56");
    });

    it("writes a Snowflake timestamp struct as a date", () => {
      const { row, typeOf } = firstRow({
        at: arrow.vectorFromArray(
          [{ epoch: 1709642096n, fraction: 789_000_000 }],
          new arrow.Struct([
            new arrow.Field("epoch", new arrow.Int64()),
            new arrow.Field("fraction", new arrow.Int32())
          ])
        )
      });
      expect(getArrowExportText(row.at, typeOf("at"))).toBe("2024-03-05 12:34:56.789");
    });

    it("leaves other values at full precision", () => {
      const { row, typeOf } = firstRow({
        ratio: arrow.makeVector(new Float64Array([0.123456])),
        count: arrow.makeVector(new BigInt64Array([9007199254740993n])),
        label: arrow.vectorFromArray(["north"])
      });
      expect(getArrowExportText(row.ratio, typeOf("ratio"))).toBe("0.123456");
      expect(getArrowExportText(row.count, typeOf("count"))).toBe("9007199254740993");
      expect(getArrowExportText(row.label, typeOf("label"))).toBe("north");
    });

    it("writes a single-precision float as the table shows it", () => {
      const { row, typeOf } = firstRow({ tenth: arrow.makeVector(new Float32Array([0.1])) });
      // The cell is the double 0.10000000149011612; the column holds 0.1.
      expect(getArrowExportText(row.tenth, typeOf("tenth"))).toBe("0.1");
    });

    it("writes a null as an empty field whatever the column type", () => {
      const { typeOf } = firstRow({
        amount: decimals([1n], 2),
        day: arrow.vectorFromArray([new Date(0)], new arrow.DateDay())
      });
      expect(getArrowExportText(null, typeOf("amount"))).toBe("");
      expect(getArrowExportText(null, typeOf("day"))).toBe("");
    });
  });
});
