// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import {
  DateDay,
  Decimal,
  makeData,
  makeVector,
  Table,
  TimestampMillisecond,
  vectorFromArray
} from "apache-arrow";
import { toast } from "sonner";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

// DuckDB answers the row count with one row and every other query with the result
// table, or fails every query with `error` once a test sets one. It counts the
// connections it hands out and the ones closed again.
const queryResult = vi.hoisted(() => ({
  table: null as unknown,
  error: null as Error | null,
  opened: 0,
  closed: 0
}));
vi.mock("@/libs/duckdb", () => ({
  registerAuthenticatedParquetFile: () => Promise.resolve("result_table"),
  getDuckDB: () =>
    Promise.resolve({
      connect: () => {
        queryResult.opened += 1;
        return Promise.resolve({
          query: (sql: string) =>
            queryResult.error
              ? Promise.reject(queryResult.error)
              : Promise.resolve(
                  sql.includes("COUNT(*)") ? { toArray: () => [{ count: 1 }] } : queryResult.table
                ),
          close: () => {
            queryResult.closed += 1;
            return Promise.resolve();
          }
        });
      }
    })
}));

import { VirtualizedTable } from "./VirtualizedTable";

let downloaded: Blob | null = null;

beforeEach(() => {
  downloaded = null;
  queryResult.error = null;
  queryResult.opened = 0;
  queryResult.closed = 0;
  vi.spyOn(console, "error").mockImplementation(() => {});
  URL.createObjectURL = vi.fn((blob: Blob) => {
    downloaded = blob;
    return "blob:csv";
  });
  URL.revokeObjectURL = vi.fn();
  // jsdom does not navigate; the click on the download link is all there is to it.
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

/** A DECIMAL(18,2) column of these unscaled integers; a null is a NULL cell. */
const decimalColumn = (unscaled: (bigint | null)[]) => {
  const words = new BigInt64Array(unscaled.length * 2);
  const validity = new Uint8Array(Math.ceil(unscaled.length / 8));
  unscaled.forEach((value, i) => {
    if (value === null) return;
    words[i * 2] = value;
    validity[i >> 3] |= 1 << (i % 8);
  });
  return makeVector(
    makeData({
      type: new Decimal(2, 18, 128),
      length: unscaled.length,
      nullCount: unscaled.filter((value) => value === null).length,
      nullBitmap: validity,
      data: new Uint32Array(words.buffer)
    })
  );
};

it("exports each cell as the value the table shows, not the raw Arrow cell", async () => {
  queryResult.table = new Table({
    // 1234.50, held as the unscaled integer 123450.
    amount: decimalColumn([123450n]),
    day: vectorFromArray([new Date("2024-03-05T00:00:00Z")], new DateDay()),
    at: vectorFromArray([new Date("2024-03-05T12:34:56Z")], new TimestampMillisecond()),
    ratio: makeVector(new Float64Array([0.123456])),
    label: vectorFromArray(["north, east"])
  });

  render(<VirtualizedTable filePath='result.parquet' />);

  // On screen: scaled, readable, and rounded for display.
  expect(await screen.findByTitle("1234.50")).toBeTruthy();
  expect(screen.getByTitle("2024-03-05")).toBeTruthy();
  expect(screen.getByTitle("2024-03-05 12:34")).toBeTruthy();
  expect(screen.getByTitle("0.12")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "CSV" }));
  const csv = await waitFor(() => {
    if (!downloaded) throw new Error("no CSV downloaded yet");
    return downloaded;
  });

  // In the file: the same values, without the display rounding.
  expect(await csv.text()).toBe(
    [
      "amount,day,at,ratio,label",
      '1234.50,2024-03-05,2024-03-05 12:34:56,0.123456,"north, east"'
    ].join("\r\n")
  );
});

it("shows a NULL as an empty cell, whatever its column type", async () => {
  queryResult.table = new Table({
    amount: decimalColumn([null, 123450n]),
    day: vectorFromArray([null, new Date("2024-03-05T00:00:00Z")], new DateDay()),
    at: vectorFromArray([null, new Date("2024-03-05T12:34:56Z")], new TimestampMillisecond()),
    label: vectorFromArray([null, "north"])
  });

  render(<VirtualizedTable filePath='result.parquet' />);

  // The second row is all there, so the first row's NULLs did not fail the table...
  expect(await screen.findByTitle("north")).toBeTruthy();
  expect(screen.getByTitle("1234.50")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  // ...and each of them is an empty cell, as a NULL text cell is.
  expect(screen.queryByTitle("Invalid Date")).toBeNull();
  expect(screen.getAllByTitle("")).toHaveLength(4);

  fireEvent.click(screen.getByRole("button", { name: "CSV" }));
  const csv = await waitFor(() => {
    if (!downloaded) throw new Error("no CSV downloaded yet");
    return downloaded;
  });
  expect(await csv.text()).toBe(
    ["amount,day,at,label", ",,,", "1234.50,2024-03-05,2024-03-05 12:34:56,north"].join("\r\n")
  );
});

it("closes each DuckDB connection it opens, to load and to export", async () => {
  queryResult.table = new Table({ label: vectorFromArray(["north"]) });

  render(<VirtualizedTable filePath='result.parquet' />);
  await screen.findByTitle("north");
  expect(queryResult.opened).toBe(1);
  expect(queryResult.closed).toBe(1);

  fireEvent.click(screen.getByRole("button", { name: "CSV" }));
  await waitFor(() => expect(downloaded).not.toBeNull());
  expect(queryResult.opened).toBe(2);
  expect(queryResult.closed).toBe(2);
});

it("closes its connection when the data fails to load", async () => {
  queryResult.error = new Error("Out of memory");

  render(<VirtualizedTable filePath='result.parquet' />);

  expect((await screen.findByRole("alert")).textContent).toContain("Out of memory");
  expect(queryResult.opened).toBe(1);
  expect(queryResult.closed).toBe(1);
});

it("tells the user when the export fails, and closes its connection", async () => {
  queryResult.table = new Table({ label: vectorFromArray(["north"]) });

  render(<VirtualizedTable filePath='result.parquet' />);
  await screen.findByTitle("north");

  queryResult.error = new Error("Out of memory");
  fireEvent.click(screen.getByRole("button", { name: "CSV" }));

  await waitFor(() => expect(toast.error).toHaveBeenCalledWith("Failed to download CSV"));
  expect(downloaded).toBeNull();
  expect(queryResult.opened).toBe(2);
  expect(queryResult.closed).toBe(2);
  // The table itself is untouched: only the export failed.
  expect(screen.getByTitle("north")).toBeTruthy();
});
