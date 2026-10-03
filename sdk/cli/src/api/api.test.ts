/**
 * The request-shaping layer: paths, fields, pagination and output.
 *
 * Weighted towards the places where a silent wrong answer is possible — a path
 * that 404s for a reason nobody can see, a field that arrives as the wrong JSON
 * type, a `--paginate` that stops early and reads as complete.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { parseDuration } from "./cache.js";
import { paramsToQuery, parseFields, parseTypedValue } from "./fields.js";
import { toMarkdown } from "./output.js";
import { hasLinkHeader, linkNext, paginate, readPage, withOffset, withPage } from "./paginate.js";
import {
  isExternalSurface,
  normalizePath,
  placeholdersIn,
  substitutePlaceholders
} from "./paths.js";

describe("normalizePath", () => {
  it("puts a bare path under /api", () => {
    expect(normalizePath("user")).toBe("/api/user");
    expect(normalizePath("/user")).toBe("/api/user");
    expect(normalizePath("projects/x/query")).toBe("/api/projects/x/query");
  });

  it("keeps a path that already names /api", () => {
    expect(normalizePath("/api/user")).toBe("/api/user");
    expect(normalizePath("api/customer-apps/oxy-access")).toBe("/api/customer-apps/oxy-access");
  });

  /**
   * THE BUG THIS FIXES, and the reason this test is worth its line count.
   *
   * `normalize_path` in the Rust `oxy api` tests only for an `api` first
   * segment, so the API-key surface — documented in its own `--help` as
   * `oxy api /external/api/<workspace_id>/sql/query` — normalised to
   * `/api/external/api/…` and 404'd. The documented example could never have
   * worked.
   */
  it("does not double-prefix a top-level mount that is not /api", () => {
    expect(normalizePath("/external/api/w/sql/query")).toBe("/external/api/w/sql/query");
    expect(normalizePath("external/api/w/sql/query")).toBe("/external/api/w/sql/query");
    expect(normalizePath("/apidoc/openapi.json")).toBe("/apidoc/openapi.json");
    expect(normalizePath("/healthz")).toBe("/healthz");
  });
});

describe("isExternalSurface", () => {
  it("picks the API-key surface from the path, so no caller has to know", () => {
    expect(isExternalSurface("/external/api/w/sql/query")).toBe(true);
    expect(isExternalSurface("/api/user")).toBe(false);
    // Not a prefix match on `/external` alone — a hypothetical `/externalise`
    // must not be read as the key surface.
    expect(isExternalSurface("/externalish/api")).toBe(false);
  });
});

describe("placeholders", () => {
  it("substitutes from context", () => {
    expect(substitutePlaceholders("/api/{org}/workspaces", { org: "poke-house" })).toBe(
      "/api/poke-house/workspaces"
    );
  });

  it("url-encodes a value so a slug with a slash cannot forge a path segment", () => {
    expect(substitutePlaceholders("/api/{org}/x", { org: "a/b" })).toBe("/api/a%2Fb/x");
  });

  /**
   * An unresolved placeholder must ERROR, never pass through as a literal.
   * Sent to the server it becomes a 404 about a workspace literally named
   * `{workspace}`, and the caller has to work backwards from that to "the
   * context did not resolve".
   */
  it("refuses to send an unresolved placeholder", () => {
    expect(() => substitutePlaceholders("/api/{workspace}/threads", {})).toThrow(
      /could not resolve/
    );
  });

  it("names an unknown placeholder rather than silently dropping it", () => {
    expect(() => substitutePlaceholders("/api/{nonsense}/x", {})).toThrow(/unknown placeholder/);
  });

  it("reports which placeholders a path uses", () => {
    expect(placeholdersIn("/api/{org}/{workspace}/threads")).toEqual(["org", "workspace"]);
  });
});

describe("typed fields", () => {
  it("keeps JSON types, matching gh api's -F", () => {
    expect(parseTypedValue("true")).toBe(true);
    expect(parseTypedValue("false")).toBe(false);
    expect(parseTypedValue("null")).toBe(null);
    expect(parseTypedValue("123")).toBe(123);
    expect(parseTypedValue('["a","b"]')).toEqual(["a", "b"]);
  });

  /**
   * A bare UUID is not valid JSON. Erroring on it would make `-F` unusable for
   * the single most common value in this API.
   */
  it("falls back to a string for a bare UUID", () => {
    const uuid = "5ce5c011-1234-4abc-9def-0123456789ab";
    expect(parseTypedValue(uuid)).toBe(uuid);
  });

  it("merges -f and -F, with the typed value winning a key clash", () => {
    const { params } = parseFields(["promote=true"], ["promote=true"]);
    expect(params.promote).toBe(true);
  });

  it("accumulates a key ending in [] into an array", () => {
    const { params } = parseFields([], ["ids[]=a", "ids[]=b"]);
    expect(params.ids).toEqual(["a", "b"]);
  });

  it("names the flag when the = is missing", () => {
    expect(() => parseFields(["bad"], [])).toThrow(/--raw-field/);
    expect(() => parseFields([], ["bad"])).toThrow(/--field/);
  });

  it("reports absence separately from an empty object", () => {
    expect(parseFields([], []).present).toBe(false);
    expect(parseFields(["a="], []).present).toBe(true);
  });
});

describe("paramsToQuery", () => {
  it("repeats a key for an array, which is what axum's extractors read", () => {
    expect(paramsToQuery({ ids: ["a", "b"] })).toBe("ids=a&ids=b");
  });

  /** `null` sent as the four letters "null" is read as a value, not absence. */
  it("drops a null rather than sending the word", () => {
    expect(paramsToQuery({ a: null, b: "1" })).toBe("b=1");
  });

  it("encodes values", () => {
    expect(paramsToQuery({ sql: "select 1" })).toBe("sql=select+1");
  });
});

describe("parseDuration", () => {
  it("accepts gh's spelling", () => {
    expect(parseDuration("30s")).toBe(30_000);
    expect(parseDuration("5m")).toBe(300_000);
    expect(parseDuration("2h")).toBe(7_200_000);
  });

  /** A bare number meaning seconds in one tool and minutes in another is the
   * ambiguity the suffix removes, so it is refused rather than assumed. */
  it("refuses a bare number", () => {
    expect(() => parseDuration("60")).toThrow(/expected a duration/);
  });
});

describe("toMarkdown", () => {
  it("renders an array of objects as a table", () => {
    const md = toMarkdown([
      { id: 1, name: "a" },
      { id: 2, name: "b" }
    ]);
    expect(md).toContain("| id | name |");
    expect(md).toContain("| 1 | a |");
  });

  it("finds the rows inside a list response", () => {
    const md = toMarkdown({ threads: [{ id: 1 }], pagination: { page: 1 } });
    expect(md).toContain("| id |");
  });

  it("keeps first-seen key order, so the identifying field stays first", () => {
    const md = toMarkdown([{ id: 1, zzz: 2 }, { aaa: 3 }]);
    expect(md?.split("\n")[0]).toBe("| id | zzz | aaa |");
  });

  /** A nested value must still be the data — a blank cell reads as an empty
   * field, which is how a caller concludes something is unset when it is not. */
  it("renders a nested value as compact JSON rather than [object Object]", () => {
    const md = toMarkdown([{ meta: { a: 1 } }]);
    expect(md).toContain('{"a":1}');
  });

  it("gives up on something that is not table-shaped", () => {
    expect(toMarkdown("a string")).toBeUndefined();
    expect(toMarkdown([1, 2, 3])).toBeUndefined();
    expect(toMarkdown([])).toBeUndefined();
  });
});

describe("pagination", () => {
  it("reads the has_next shape thread.rs uses", () => {
    const page = readPage({ threads: [{ id: 1 }], pagination: { has_next: true } }, 1);
    expect(page.rowsKey).toBe("threads");
    expect(page.hasMore).toBe(true);
  });

  it("reads the bare has_more shape other handlers use", () => {
    expect(readPage({ commits: [], has_more: true }, 1).hasMore).toBe(true);
    expect(readPage({ commits: [], has_more: false }, 1).hasMore).toBe(false);
  });

  it("falls back to page < total_pages", () => {
    expect(readPage({ rows: [], pagination: { total_pages: 3 } }, 1).hasMore).toBe(true);
    expect(readPage({ rows: [], pagination: { total_pages: 3 } }, 3).hasMore).toBe(false);
  });

  /**
   * THE BUG THIS PINS. Treating a full bare array as "there is probably more"
   * loops to the page cap against any endpoint that ignores `?page` — it
   * returns the same array every time — and merges a hundred copies of the
   * same rows. A hundred duplicates presented as a result is far worse than a
   * missed page, which at least shows up as a short list.
   */
  it("treats a bare array as ONE page, never as 'maybe more'", () => {
    expect(readPage([{ id: 1 }, { id: 2 }], 1).hasMore).toBe(false);
    expect(readPage([], 1).hasMore).toBe(false);
  });

  it("recognises no pagination signal at all as one page", () => {
    expect(readPage({ threads: [{ id: 1 }] }, 1).hasMore).toBe(false);
  });

  /**
   * `hasMore: false` is TWO different answers and `--paginate` has to tell them
   * apart. "The server said this is the last page" is complete; "the server
   * said nothing" is one page of an endpoint that may well have more — which is
   * the whole admin surface, where `page`/`page_size` and `limit`/`offset`
   * endpoints answer with a bare array. Only the second is worth warning about,
   * so a false `has_next` must NOT be reported as a missing signal.
   */
  it("separates 'the server said no more' from 'the server said nothing'", () => {
    expect(readPage({ threads: [], pagination: { has_next: false } }, 1).signal).toBe(
      "pagination.has_next"
    );
    expect(readPage({ commits: [], has_more: false }, 1).signal).toBe("has_more");
    expect(readPage({ rows: [], pagination: { total_pages: 3 } }, 3).signal).toBe(
      "pagination.total_pages"
    );

    // The shapes that carry nothing to read: a bare array, and an object whose
    // only content is the rows.
    expect(readPage([{ id: 1 }], 1).signal).toBeUndefined();
    expect(readPage({ threads: [{ id: 1 }] }, 1).signal).toBeUndefined();
  });

  it("honours an explicit --paginate-key over the first-array guess", () => {
    const payload = { meta: [1], threads: [{ id: 1 }] };
    expect(readPage(payload, 1).rowsKey).toBe("meta");
    expect(readPage(payload, 1, "threads").rowsKey).toBe("threads");
  });

  it("follows Link: rel=next when a server sends one", () => {
    expect(linkNext({ link: '<https://x/api/a?page=2>; rel="next"' })).toBe(
      "https://x/api/a?page=2"
    );
    expect(linkNext({ link: '<https://x/a>; rel="prev"' })).toBeUndefined();
    expect(linkNext({})).toBeUndefined();
  });

  /**
   * THE FALSE POSITIVE THIS PREVENTS.
   *
   * The last page of a paginated endpoint carries no `rel="next"` — that is
   * what makes it the last page. Deciding "did this endpoint say anything about
   * pagination" from `linkNext` therefore answers NO for a single-page result
   * from `admin/audit`, `admin/users` or `/assume/history`, and `--paginate`
   * warns "this is ONE page, not necessarily every row" about exactly the
   * endpoints that answered completely and correctly. The handlers emit
   * `rel="first"` on every page so this question has an answer.
   */
  it("counts any Link as a pagination signal, not only rel=next", () => {
    expect(hasLinkHeader({ link: '</api/admin/audit>; rel="first"' })).toBe(true);
    expect(linkNext({ link: '</api/admin/audit>; rel="first"' })).toBeUndefined();

    expect(
      hasLinkHeader({
        link: '</api/admin/audit?offset=100>; rel="next", </api/admin/audit>; rel="first"'
      })
    ).toBe(true);
    expect(hasLinkHeader({})).toBe(false);
  });

  /** A two-link header must still yield the next one. */
  it("picks rel=next out of a header carrying several links", () => {
    expect(
      linkNext({
        link: '</api/admin/audit?offset=100>; rel="next", </api/admin/audit>; rel="first"'
      })
    ).toBe("/api/admin/audit?offset=100");
  });

  /**
   * Oxy's own `Link` is a RELATIVE reference — `oxy_app_core::pagination` emits
   * one deliberately, because reconstructing an absolute URL from `Host` behind
   * the proxy and the subdomain dispatch is how you emit a link to the wrong
   * host. `paginate()` feeds it straight to `buildUrl`, which concatenates onto
   * the target, so the relative form has to survive `linkNext` intact.
   */
  it("accepts the relative Link the Oxy handlers emit", () => {
    expect(linkNext({ link: '</api/admin/users?search=acme&page=1>; rel="next"' })).toBe(
      "/api/admin/users?search=acme&page=1"
    );
  });

  it("replaces rather than appends ?page, so it cannot accumulate", () => {
    expect(withPage("/api/x?page=1&limit=5", 2)).toBe("/api/x?page=2&limit=5");
    expect(withPage("/api/x", 2)).toBe("/api/x?page=2");
  });

  /**
   * The app listings (`/api/admin/apps`, `/api/customer-apps`) page by
   * `limit`/`offset` and answer `{items, next_offset}`. A number means "ask
   * again from here"; `null` is the server saying this was the last page, which
   * is an answer and must not read as "no pagination signal".
   */
  it("reads the next_offset shape the app listings use", () => {
    const more = readPage({ items: [{ id: 1 }], next_offset: 50 }, 1);
    expect(more).toMatchObject({
      rowsKey: "items",
      hasMore: true,
      signal: "next_offset",
      nextOffset: 50
    });

    const last = readPage({ items: [{ id: 2 }], next_offset: null }, 2);
    expect(last.hasMore).toBe(false);
    expect(last.signal).toBe("next_offset");
    expect(last.nextOffset).toBeUndefined();
  });

  it("replaces ?offset and keeps the caller's limit", () => {
    expect(withOffset("/api/customer-apps?limit=20&offset=20", 40)).toBe(
      "/api/customer-apps?limit=20&offset=40"
    );
    expect(withOffset("/api/customer-apps", 50)).toBe("/api/customer-apps?offset=50");
  });
});

describe("paginate — an endpoint that pages by next_offset", () => {
  const TARGET = "https://oxy.test";

  /** Serve `pages` keyed by the `offset` the request carried; record every URL asked for. */
  function stubListing(pages: Record<string, unknown>): string[] {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL) => {
        const url = String(input);
        urls.push(url.replace(TARGET, ""));
        const offset = new URL(url).searchParams.get("offset") ?? "0";
        return new Response(JSON.stringify(pages[offset] ?? { error: `no page at ${offset}` }), {
          status: pages[offset] ? 200 : 404,
          headers: { "content-type": "application/json" }
        });
      })
    );
    return urls;
  }

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  /**
   * THE BUG THIS PINS. `next_offset` was not a signal `readPage` knew, so
   * `oxyc api api/customer-apps --paginate` returned the first 50 apps of 58
   * and warned only that there was "no pagination signal" — and asking for
   * `?page=2` instead would not have helped, because the endpoint ignores it.
   */
  it("follows next_offset to the last page and merges every row", async () => {
    const urls = stubListing({
      "0": { items: [{ id: "a" }, { id: "b" }], next_offset: 2 },
      "2": { items: [{ id: "c" }, { id: "d" }], next_offset: 4 },
      "4": { items: [{ id: "e" }], next_offset: null }
    });
    const warn = vi.spyOn(process.stderr, "write").mockImplementation(() => true);

    const merged = JSON.parse(
      await paginate({ target: TARGET, path: "/api/customer-apps?limit=2", method: "GET" })
    );

    expect(merged.items.map((row: { id: string }) => row.id)).toEqual(["a", "b", "c", "d", "e"]);
    expect(merged.next_offset).toBeNull();
    expect(urls).toEqual([
      "/api/customer-apps?limit=2",
      "/api/customer-apps?limit=2&offset=2",
      "/api/customer-apps?limit=2&offset=4"
    ]);
    // A listing that reported its own last page is complete: nothing to warn about.
    expect(warn).not.toHaveBeenCalled();
  });

  /** A server that keeps answering the same offset must not be asked a hundred times. */
  it("stops, and says the result is incomplete, when next_offset does not advance", async () => {
    const urls = stubListing({
      "0": { items: [{ id: "a" }], next_offset: 1 },
      "1": { items: [{ id: "b" }], next_offset: 1 }
    });
    const stderr: string[] = [];
    vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
      stderr.push(String(chunk));
      return true;
    });

    const merged = JSON.parse(
      await paginate({ target: TARGET, path: "/api/customer-apps", method: "GET" })
    );

    expect(merged.items).toHaveLength(2);
    expect(urls).toHaveLength(2);
    expect(stderr.join("")).toMatch(/INCOMPLETE/);
  });
});

describe("toMarkdown — the columnar shapes this API actually returns", () => {
  /**
   * `/sql/query` answers with header-row-first arrays by default. It is the
   * most common data response in the API and is not an array of objects, so
   * without an explicit case the generic path rejected it and `--md` — asked
   * for exactly this — printed raw JSON.
   */
  it("renders header-row-first arrays", () => {
    const md = toMarkdown([
      ["id", "name"],
      ["1", "ada"],
      ["2", "grace"]
    ]);
    expect(md).toContain("| id | name |");
    expect(md).toContain("| 1 | ada |");
    expect(md).toContain("| 2 | grace |");
    // The header row must not also appear as data.
    expect(md?.match(/\| id \| name \|/g)).toHaveLength(1);
  });

  it("renders the {columns, rows} shape /projects/*/query returns", () => {
    const md = toMarkdown({ columns: ["a", "b"], rows: [[1, 2]], truncated: false });
    expect(md).toContain("| a | b |");
    expect(md).toContain("| 1 | 2 |");
  });

  it("treats a header with no rows as an empty table, not as one data row", () => {
    const md = toMarkdown([["id", "name"]]);
    expect(md).toContain("| id | name |");
    expect(md).not.toContain("| id | name |\n| --- | --- |\n| id |");
  });

  it("pads a short row rather than dropping the column", () => {
    const md = toMarkdown([["a", "b"], ["1"]]);
    expect(md).toContain("| 1 |  |");
  });

  /** An array of plain strings is not columnar — it must not become a table. */
  it("leaves a flat array alone", () => {
    expect(toMarkdown(["a", "b"])).toBeUndefined();
  });
});

describe("columnar rendering — the three ways it got results wrong", () => {
  /**
   * `SELECT a.id, b.id` gives two columns called `id`. Building each row as an
   * object keyed by header name collapses them, so both columns print the
   * second one's value — silently, and identically, so nothing looks wrong.
   */
  it("keeps duplicate column names as separate columns", () => {
    const md = toMarkdown([
      ["id", "id"],
      ["left", "right"]
    ]);
    expect(md).toContain("| left | right |");
    expect(md).not.toContain("| right | right |");
  });

  /**
   * A payload carrying `columns` and `rows` where the rows are OBJECTS is a
   * different shape; indexing objects positionally renders a table of empty
   * cells. Falling through to the generic path gets it right.
   */
  it("falls through when `rows` are objects rather than arrays", () => {
    const md = toMarkdown({ columns: ["a"], rows: [{ a: 1 }, { a: 2 }] });
    expect(md).toContain("| a |");
    expect(md).toContain("| 1 |");
    expect(md).toContain("| 2 |");
  });

  it("still renders the real {columns, rows} shape", () => {
    const md = toMarkdown({ columns: ["a", "b"], rows: [[1, 2]] });
    expect(md).toContain("| a | b |");
    expect(md).toContain("| 1 | 2 |");
  });

  /**
   * The zero-row case used to hand-write markdown pipes while the row-bearing
   * path went through `table()` — so on a terminal the same query rendered
   * aligned with rows and pipe-delimited without.
   *
   * ASSERTED ON BYTES, and under BOTH streams. Comparing "does the empty form
   * have pipes" against "does the populated form have pipes" cannot fail: both
   * are computed in one process off one `stdoutIsTty()`, and under vitest that
   * is always false — so both were markdown, both were true, and the
   * hand-written version passed it too. The inconsistency only existed when
   * stdout WAS a tty, which is the one condition that test never created.
   */
  it("renders an empty result as markdown when piped", () => {
    expect(toMarkdown([["id", "name"]])).toBe("| id | name |\n| --- | --- |");
  });

  it("renders an empty result WITHOUT pipes on a terminal, like a populated one", () => {
    const original = process.stdout.isTTY;
    try {
      Object.defineProperty(process.stdout, "isTTY", { value: true, configurable: true });
      const empty = toMarkdown([["id", "name"]]);
      const populated = toMarkdown([
        ["id", "name"],
        ["1", "ada"]
      ]);
      expect(empty).not.toContain("|");
      expect(populated).not.toContain("|");
      // The header is still named, which is the whole reason the empty case
      // renders at all: it says what was queried as well as that it was empty.
      expect(empty).toContain("id");
      expect(empty).toContain("name");
    } finally {
      Object.defineProperty(process.stdout, "isTTY", { value: original, configurable: true });
    }
  });
});
