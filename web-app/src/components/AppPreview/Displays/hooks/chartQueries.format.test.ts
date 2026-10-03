import { makeVector, Table, type Vector, vectorFromArray } from "apache-arrow";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/libs/duckdb", () => ({ getDuckDB: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));

import { resolveValueFormat } from "./chartQueries";

/** A connection whose source table has this one column. */
const connectionWith = (column: string, values: Vector) => {
  const query = vi.fn().mockResolvedValue(new Table({ [column]: values }));
  return { connection: { query } as never, query };
};

const amounts = () => makeVector(new Float64Array([12.5]));
const integers = () => makeVector(new Int32Array([7]));

describe("resolveValueFormat", () => {
  it("infers currency for a numeric column whose name says money", async () => {
    const { connection, query } = connectionWith("total_sales", amounts());

    expect(await resolveValueFormat(connection, "orders", "total_sales")).toBe("currency");
    // The type is read from the source column, without fetching a row.
    expect(query).toHaveBeenCalledWith('SELECT "total_sales" FROM "orders" LIMIT 0');
  });

  it.each(["payment_count", "payment_id", "discount_rate", "payment_year"])(
    "puts no currency on %s, a number that is not an amount of money",
    async (column) => {
      const { connection } = connectionWith(column, integers());
      expect(await resolveValueFormat(connection, "orders", column)).toBeUndefined();
    }
  );

  it("puts no currency on a column that is not numeric, whatever its name", async () => {
    // DuckDB sums a boolean, so such a chart renders: its total is a tally.
    const flags = connectionWith("fee_paid", vectorFromArray([true]));
    expect(await resolveValueFormat(flags.connection, "orders", "fee_paid")).toBeUndefined();

    // The name alone is what the charts used to go by.
    const numbers = connectionWith("fee_paid", amounts());
    expect(await resolveValueFormat(numbers.connection, "orders", "fee_paid")).toBe("currency");
  });

  it("keeps the format the app declares, without looking at the column", async () => {
    const { connection, query } = connectionWith("payment_count", integers());

    expect(await resolveValueFormat(connection, "orders", "payment_count", "currency")).toBe(
      "currency"
    );
    expect(await resolveValueFormat(connection, "orders", "total_sales", "number")).toBe("number");
    expect(query).not.toHaveBeenCalled();
  });

  it("quotes a column name that needs it", async () => {
    const { connection, query } = connectionWith('Total "net" sales', amounts());

    expect(await resolveValueFormat(connection, "orders", 'Total "net" sales')).toBe("currency");
    expect(query).toHaveBeenCalledWith('SELECT "Total ""net"" sales" FROM "orders" LIMIT 0');
  });
});
