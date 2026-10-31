// The page's three checks. Every failure must reject with an error that starts
// with the check's own name, because that name becomes `data-canary-failed`.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AIRHOUSE_DATABASE, AIRHOUSE_READS } from "../functions/airhouse-reads";
import {
  airhouseRequested,
  checkAirhouseQueryRoute,
  checkEcho,
  checkSdkQuery,
  checkSqlQueryRoute,
  DATABASE,
  SELECT_ONE
} from "./checks";

type FetchArgs = [input: string, init: RequestInit];

function respond(status: number, payload: unknown) {
  return vi.fn(async (..._args: FetchArgs) => new Response(JSON.stringify(payload), { status }));
}

describe("checkEcho", () => {
  it("passes when the function sends the nonce back", async () => {
    await expect(checkEcho(async (body) => ({ ok: true, echo: body }))).resolves.toBeUndefined();
  });

  it("fails on a nonce mismatch", async () => {
    await expect(
      checkEcho(async () => ({ ok: true, echo: { nonce: "someone-elses-nonce" } }))
    ).rejects.toThrow(/^echo: the function did not send the nonce back$/);
  });

  it("fails under its name when the invoke rejects", async () => {
    await expect(
      checkEcho(async () => {
        throw new Error("401 unauthorized");
      })
    ).rejects.toThrow(/^echo: 401 unauthorized$/);
  });
});

describe("checkSqlQueryRoute", () => {
  beforeEach(() => {
    vi.stubGlobal("__OXY_APP__", { projectId: "proj-1", apiBaseUrl: "https://oxy.example.test" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends the request bookkeeping's useSqlQuery sends, and passes on one row", async () => {
    const fetchMock = respond(200, [["one"], [1]]);
    vi.stubGlobal("fetch", fetchMock);
    await expect(checkSqlQueryRoute()).resolves.toBeUndefined();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://oxy.example.test/api/proj-1/sql/query");
    expect(init).toMatchObject({ method: "POST", credentials: "include" });
    expect(JSON.parse(String(init.body))).toEqual({ sql: SELECT_ONE, database: DATABASE });
  });

  it("passes on the { columns, rows } payload too", async () => {
    vi.stubGlobal("fetch", respond(200, { columns: ["one"], rows: [[1]] }));
    await expect(checkSqlQueryRoute()).resolves.toBeUndefined();
  });

  it("fails on a non-2xx answer, even one that carries a row", async () => {
    vi.stubGlobal("fetch", respond(500, [["one"], [1]]));
    await expect(checkSqlQueryRoute()).rejects.toThrow(/^sql_query_route: query failed \(500\)/);
  });

  it.each([
    ["positional", [["one"]]],
    ["{ columns, rows }", { columns: ["one"], rows: [] }]
  ] as Array<[string, unknown]>)("fails on zero rows: %s", async (_label, payload) => {
    vi.stubGlobal("fetch", respond(200, payload));
    await expect(checkSqlQueryRoute()).rejects.toThrow(
      /^sql_query_route: expected one row with one = 1, got 0 rows$/
    );
  });

  it("fails when the request itself rejects", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      })
    );
    await expect(checkSqlQueryRoute()).rejects.toThrow(/^sql_query_route: Failed to fetch$/);
  });

  it("fails when the page has no injected __OXY_APP__", async () => {
    vi.stubGlobal("__OXY_APP__", undefined);
    vi.stubGlobal("fetch", respond(200, [["one"], [1]]));
    await expect(checkSqlQueryRoute()).rejects.toThrow(
      /^sql_query_route: window\.__OXY_APP__\.projectId is missing$/
    );
  });
});

describe("checkSdkQuery", () => {
  it("passes on one row", async () => {
    await expect(checkSdkQuery(Promise.resolve([{ one: 1 }]))).resolves.toBeUndefined();
  });

  it("fails when the query errors", async () => {
    await expect(
      checkSdkQuery(Promise.reject(new Error("failed to build connector")))
    ).rejects.toThrow(/^sdk_query: failed to build connector$/);
  });

  it("fails on zero rows", async () => {
    await expect(checkSdkQuery(Promise.resolve([]))).rejects.toThrow(
      /^sdk_query: expected one row with one = 1, got 0 rows$/
    );
  });
});

/** What `/api/projects/{id}/query` answers for one of `AIRHOUSE_READS`: `n` = 1…N, BIGINT as text. */
function airhouseAnswer(sql: string): { columns: string[]; rows: unknown[][]; truncated: boolean } {
  const read = AIRHOUSE_READS.find((r) => r.sql === sql);
  if (!read) throw new Error(`not an Airhouse read: ${sql}`);
  const rows = Array.from({ length: read.rows }, (_, i) => [String(i + 1)]);
  // An empty result has no columns: `json_objects_to_table` derives them from the rows.
  return { columns: read.rows === 0 ? [] : ["n"], rows, truncated: false };
}

describe("checkAirhouseQueryRoute", () => {
  beforeEach(() => {
    vi.stubGlobal("__OXY_APP__", { projectId: "proj-1", apiBaseUrl: "https://oxy.example.test" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends each read to the SDK's /query route on canary_airhouse, past its cache, and passes", async () => {
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => {
      const { sql } = JSON.parse(String(init.body)) as { sql: string };
      return new Response(JSON.stringify(airhouseAnswer(sql)), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    await expect(checkAirhouseQueryRoute()).resolves.toBeUndefined();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(
      AIRHOUSE_READS.map(() => "https://oxy.example.test/api/projects/proj-1/query?refresh=1")
    );
    expect(fetchMock.mock.calls.map(([, init]) => JSON.parse(String(init.body)))).toEqual(
      AIRHOUSE_READS.map((read) => ({ sql: read.sql, database: AIRHOUSE_DATABASE }))
    );
    expect(fetchMock.mock.calls[0][1]).toMatchObject({ method: "POST", credentials: "include" });
  });

  it("fails naming the case on the 500 #3791 answered every empty result with", async () => {
    vi.stubGlobal(
      "fetch",
      respond(500, { error: "failed to convert query results" })
    );
    await expect(checkAirhouseQueryRoute()).rejects.toThrow(
      /^airhouse_query_route: zero rows: query failed \(500\): .*failed to convert query results/
    );
  });

  it("fails when the 2,500-row read comes back a row short", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url: string, init: RequestInit) => {
        const answer = airhouseAnswer((JSON.parse(String(init.body)) as { sql: string }).sql);
        if (answer.rows.length > 1) answer.rows.pop();
        return new Response(JSON.stringify(answer), { status: 200 });
      })
    );
    await expect(checkAirhouseQueryRoute()).rejects.toThrow(
      /^airhouse_query_route: 2500 rows: expected 2500 rows, n = 1 to 2500 once each; got 2499 rows, 2499 distinct$/
    );
  });
});

describe("airhouseRequested", () => {
  it.each([
    ["?airhouse=1", true],
    ["?x=2&airhouse=1", true],
    ["", false],
    ["?airhouse=0", false],
    ["?airhouse", false]
  ] as Array<[string, boolean]>)("%s → %s", (search, expected) => {
    expect(airhouseRequested(search)).toBe(expected);
  });
});
