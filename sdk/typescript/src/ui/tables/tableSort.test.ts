// @vitest-environment node
//
// Sorting, and the one rule that matters: absence is never a value. The reference's
// `src/lib/tableSort.test.ts`, with supply-chain's extra cases (pokehouse-oxy
// `apps/supply-chain/src/lib/tableSort.test.ts`: both directions in full, NaN among nulls).

import { describe, expect, it } from "vitest";
import { formatSortParam, nextSort, parseSortParam, sortRows } from "./tableSort";

const KEYS = ["store", "cost_gap"] as const;

describe("clicking a heading", () => {
  it("cycles first direction, other direction, then back to no sort", () => {
    let state = nextSort<(typeof KEYS)[number]>(null, "cost_gap", "desc");
    expect(state).toEqual({ key: "cost_gap", dir: "desc" });
    state = nextSort(state, "cost_gap", "desc");
    expect(state).toEqual({ key: "cost_gap", dir: "asc" });
    // The third click restores the table's own considered order.
    expect(nextSort(state, "cost_gap", "desc")).toBeNull();
  });

  it("cycles the same way for a column that starts ascending", () => {
    let state = nextSort<(typeof KEYS)[number]>(null, "store", "asc");
    expect(state).toEqual({ key: "store", dir: "asc" });
    state = nextSort(state, "store", "asc");
    expect(state).toEqual({ key: "store", dir: "desc" });
    expect(nextSort(state, "store", "asc")).toBeNull();
  });

  it("starts a different column at its own first direction", () => {
    expect(nextSort({ key: "cost_gap", dir: "asc" }, "store", "asc")).toEqual({
      key: "store",
      dir: "asc"
    });
  });
});

describe("rows with nothing in the sorted column", () => {
  const rows = [
    { name: "a", v: 3 as number | null },
    { name: "b", v: null },
    { name: "c", v: 1 },
    { name: "d", v: Number.NaN }
  ];

  it("go last in BOTH directions", () => {
    for (const dir of ["asc", "desc"] as const) {
      const out = sortRows(rows, (r) => r.v, dir).map((r) => r.name);
      expect(out.slice(2)).toEqual(["b", "d"]);
    }
  });

  it("keeps the incoming order among themselves", () => {
    expect(sortRows(rows, (r) => r.v, "asc").map((r) => r.name)).toEqual(["c", "a", "b", "d"]);
  });

  it("sorts the valued rows each way, and strings by locale", () => {
    const five = [
      { name: "a", v: 3 as number | null },
      { name: "b", v: null },
      { name: "c", v: 1 },
      { name: "d", v: null },
      { name: "e", v: 2 }
    ];
    const order = (rs: { name: string }[]) => rs.map((r) => r.name).join("");
    expect(order(sortRows(five, (r) => r.v, "asc"))).toBe("ceabd");
    expect(order(sortRows(five, (r) => r.v, "desc"))).toBe("aecbd");
    const names = [{ n: "Örebro" }, { n: "Austin" }, { n: "Zurich" }];
    expect(sortRows(names, (r) => r.n, "asc").map((r) => r.n)).toEqual([
      "Austin",
      "Örebro",
      "Zurich"
    ]);
  });

  it("leaves the input untouched", () => {
    const before = rows.map((r) => r.name);
    sortRows(rows, (r) => r.v, "desc");
    expect(rows.map((r) => r.name)).toEqual(before);
  });
});

describe("the sort in the URL", () => {
  it("writes only what differs from the table's own order", () => {
    expect(formatSortParam(null)).toBeNull();
    expect(formatSortParam({ key: "cost_gap", dir: "asc" })).toBe("cost_gap");
    expect(formatSortParam({ key: "cost_gap", dir: "desc" })).toBe("-cost_gap");
  });

  it("treats an unknown column as no sort at all", () => {
    expect(parseSortParam("-cost_gap", KEYS)).toEqual({ key: "cost_gap", dir: "desc" });
    expect(parseSortParam("store", KEYS)).toEqual({ key: "store", dir: "asc" });
    expect(parseSortParam("whatever", KEYS)).toBeNull();
    expect(parseSortParam(null, KEYS)).toBeNull();
  });

  it("reads back what it writes", () => {
    for (const s of [
      { key: "store", dir: "asc" } as const,
      { key: "cost_gap", dir: "desc" } as const
    ])
      expect(parseSortParam(formatSortParam(s), KEYS)).toEqual(s);
  });
});
