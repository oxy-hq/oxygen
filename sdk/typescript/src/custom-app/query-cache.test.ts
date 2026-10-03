import { beforeEach, describe, expect, it, vi } from "vitest";
import { __clearQueryCache, getCached, queryKey, sharedQuery } from "./query-cache";

function fakeFetcher(payload: unknown) {
  return vi.fn(
    async () =>
      new Response(JSON.stringify(payload), {
        status: 200,
        headers: { "content-type": "application/json" }
      })
  );
}

describe("query-cache", () => {
  beforeEach(() => __clearQueryCache());

  it("queryKey distinguishes project, db, sql", () => {
    expect(queryKey("p1", "db", "select 1")).not.toBe(queryKey("p2", "db", "select 1"));
    expect(queryKey("p1", "db", "select 1")).not.toBe(queryKey("p1", "db2", "select 1"));
    expect(queryKey("p1", "db", "select 1")).not.toBe(queryKey("p1", "db", "select 2"));
    expect(queryKey("p1", "db", "select 1")).toBe(queryKey("p1", "db", "select 1"));
  });

  it("dedupes concurrent identical queries into one fetch", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    const [r1, r2] = await Promise.all([
      sharedQuery(f, "p1", "select 1", undefined),
      sharedQuery(f, "p1", "select 1", undefined)
    ]);
    expect(f).toHaveBeenCalledTimes(1);
    expect(r1).toEqual({ columns: ["a"], rows: [[1]] });
    expect(r2).toEqual(r1);
  });

  it("serves a cached result within TTL without re-fetching", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    await sharedQuery(f, "p1", "select 1", undefined); // populates cache
    const cached = getCached("p1", "select 1", undefined);
    expect(cached).toEqual({ columns: ["a"], rows: [[1]] });
    await sharedQuery(f, "p1", "select 1", undefined); // still 1 (served/deduped)
    expect(f).toHaveBeenCalledTimes(1);
  });

  it("sends params beside the SQL, not written into it", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    const sql = "select * from t where name = {{ params.name | sqlquote }}";
    await sharedQuery(f, "p1", sql, "wh", { params: { name: "x\\' OR 1=1 -- " } });
    const [path, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/api/projects/p1/query");
    expect(JSON.parse(init.body as string)).toEqual({
      sql,
      database: "wh",
      params: { name: "x\\' OR 1=1 -- " }
    });
  });

  it("sends no params field for a query that has none", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    await sharedQuery(f, "p1", "select 1", undefined);
    const [, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(JSON.parse(init.body as string)).toEqual({ sql: "select 1" });
  });

  it("treats one template with different params as different queries", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    const sql = "select {{ params.n }}";
    await sharedQuery(f, "p1", sql, undefined, { params: { n: 1 } });
    await sharedQuery(f, "p1", sql, undefined, { params: { n: 2 } });
    await sharedQuery(f, "p1", sql, undefined, { params: { n: 1 } }); // cached
    expect(f).toHaveBeenCalledTimes(2);
    expect(getCached("p1", sql, undefined, { n: 2 })).toBeDefined();
    expect(getCached("p1", sql, undefined, { n: 3 })).toBeUndefined();
    expect(getCached("p1", sql, undefined)).toBeUndefined();
  });

  it("queryKey cannot be spelled by SQL that looks like another query's params", () => {
    expect(queryKey("p1", "db", "select 1", { n: 1 })).not.toBe(
      queryKey("p1", "db", 'select 1 {"n":1}')
    );
  });

  it("force bypasses the fresh-cache short-circuit", async () => {
    const f = fakeFetcher({ columns: ["a"], rows: [[1]] });
    await sharedQuery(f, "p1", "q", undefined);
    await sharedQuery(f, "p1", "q", undefined, { force: true });
    expect(f).toHaveBeenCalledTimes(2);
  });
});
