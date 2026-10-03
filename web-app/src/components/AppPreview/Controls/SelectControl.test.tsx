// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { Table, Utf8, vectorFromArray } from "apache-arrow";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataContainer } from "@/types/app";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/api/axios", () => ({
  apiClient: { get: () => Promise.resolve({ data: new ArrayBuffer(0) }) }
}));

// DuckDB answers with a source table whose one column, `region`, holds `values`,
// or fails every query with `error` once a test sets one.
// A test that sets `held` keeps every query waiting until it resolves that promise.
const source = vi.hoisted(() => ({
  values: [] as string[],
  error: null as Error | null,
  queries: 0,
  held: null as Promise<void> | null
}));
vi.mock("@/libs/duckdb", () => ({
  getDuckDB: () =>
    Promise.resolve({
      registerFileBuffer: () => Promise.resolve(),
      connect: () =>
        Promise.resolve({
          query: async (sql: string) => {
            source.queries += 1;
            if (source.held) await source.held;
            if (source.error) throw source.error;
            // The control reads the column's name, then its distinct values as `val`.
            const column = sql.includes("DISTINCT") ? "val" : "region";
            return new Table({ [column]: vectorFromArray(source.values, new Utf8()) });
          },
          close: () => Promise.resolve()
        })
    })
}));
// Radix's select cannot open in jsdom: a native one shows the same options.
vi.mock("@/components/ui/shadcn/select", () => ({
  Select: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  SelectTrigger: ({ "aria-invalid": invalid }: { "aria-invalid"?: boolean }) => (
    <button type='button' aria-invalid={invalid}>
      trigger
    </button>
  ),
  SelectValue: () => null,
  SelectContent: ({ children }: { children: ReactNode }) => <ul>{children}</ul>,
  SelectItem: ({ children }: { children: ReactNode }) => <li>{children}</li>
}));

import { SelectControl } from "./SelectControl";

/** The control, its options read from the task result `sourceName` in `data`. */
const controlOn = (sourceName: string, data: DataContainer) => (
  <SelectControl
    control={{ name: "region", type: "select", label: "Region", source: sourceName }}
    value=''
    data={data}
    onChange={() => {}}
  />
);

const renderControl = () =>
  render(controlOn("regions", { regions: { file_path: "regions.parquet" } }));

const options = () => screen.queryAllByRole("listitem").map((item) => item.textContent);

beforeEach(() => {
  source.values = ["north", "south"];
  source.error = null;
  source.queries = 0;
  source.held = null;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("SelectControl options from a source", () => {
  it("lists the values of the source's first column", async () => {
    renderControl();

    await waitFor(() => expect(options()).toEqual(["north", "south"]));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says so when the options fail to load, rather than offering an empty list", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    source.error = new Error("Out of memory");

    renderControl();

    expect((await screen.findByRole("alert")).textContent).toBe("Failed to load options");
    expect(screen.getByRole("button").getAttribute("aria-invalid")).toBe("true");
    expect(options()).toEqual([]);
    expect(logged).toHaveBeenCalled();
  });

  it("offers an empty list without complaint when the source has no rows", async () => {
    source.values = [];

    renderControl();

    // Both queries ran, so the load is over: there was just nothing to list.
    await waitFor(() => expect(source.queries).toBe(2));
    expect(options()).toEqual([]);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("SelectControl when its source changes", () => {
  const regions = { regions: { file_path: "regions.parquet" } };

  it("offers nothing for a source that has no file to read", async () => {
    const { rerender } = render(controlOn("regions", regions));
    await waitFor(() => expect(options()).toEqual(["north", "south"]));

    // A source whose task has not produced a result, and one that is not there at all.
    rerender(controlOn("cities", { ...regions, cities: { file_path: "" } }));
    expect(options()).toEqual([]);

    rerender(controlOn("regions", regions));
    await waitFor(() => expect(options()).toEqual(["north", "south"]));
    rerender(controlOn("cities", regions));
    expect(options()).toEqual([]);
  });

  it("does not offer the previous source's options while the new one loads", async () => {
    const { rerender } = render(controlOn("regions", regions));
    await waitFor(() => expect(options()).toEqual(["north", "south"]));

    let release = () => {};
    source.held = new Promise<void>((resolve) => {
      release = resolve;
    });
    source.values = ["paris", "rome"];
    rerender(controlOn("cities", { ...regions, cities: { file_path: "cities.parquet" } }));

    await waitFor(() => expect(source.queries).toBeGreaterThan(2));
    expect(options()).toEqual([]);

    release();
    await waitFor(() => expect(options()).toEqual(["paris", "rome"]));
  });

  it("keeps its options on screen while the same source's file is read again", async () => {
    const { rerender } = render(controlOn("regions", regions));
    await waitFor(() => expect(options()).toEqual(["north", "south"]));

    // The app re-ran: a new data object, the same result file.
    let release = () => {};
    source.held = new Promise<void>((resolve) => {
      release = resolve;
    });
    rerender(controlOn("regions", { regions: { file_path: "regions.parquet" } }));

    await waitFor(() => expect(source.queries).toBeGreaterThan(2));
    expect(options()).toEqual(["north", "south"]);
    release();
  });
});
