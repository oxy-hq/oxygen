/**
 * `--paginate` — walk every page and hand back one document.
 *
 * `gh api --paginate` has an easy job: GitHub sends `Link: rel="next"` on
 * every paginated endpoint, so the client follows a URL it was given. Oxy has
 * no such header and no single pagination shape — `thread.rs` answers with
 * `{pagination:{total_pages,has_next}}`, `workspaces/ops.rs` with a bare
 * `has_more`, the app listings (`admin/apps`, `customer-apps`) with
 * `next_offset`, and plenty of list endpoints with none of them.
 *
 * So this is a HEURISTIC, and it says so in `--help` rather than pretending to
 * be a contract. It reads, in order: a `Link: rel="next"` header (free
 * correctness if one ever appears), then `next_offset`, then
 * `pagination.has_next`, then `has_more`, then `page < pagination.total_pages`.
 * Nothing recognised means one page, which is the safe direction — a missed
 * page is visible in the result, an invented one is not.
 */

import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  type ApiResponse,
  errorForResponse,
  parseJson,
  type RequestOptions,
  request
} from "./request.js";

/**
 * The ceiling on pages walked in one run.
 *
 * Not a tuning knob so much as a guard against a server whose "has more" never
 * goes false — a bug that without this turns one command into an infinite
 * request loop against production. Hitting it is REPORTED, never silent: a
 * truncated result that reads as complete is the failure this whole file is
 * shaped to avoid.
 */
const MAX_PAGES = 100;

export interface PaginateOptions extends RequestOptions {
  /** Force the field holding the rows, when the guess would be wrong. */
  paginateKey?: string;
  /** Emit an array of whole pages instead of merging them (`--slurp`). */
  slurp?: boolean;
  maxPages?: number;
}

export interface PageShape {
  /** The field the rows live under, if this looks like a list response. */
  rowsKey?: string;
  rows: unknown[];
  hasMore: boolean;
  /**
   * Which rule decided `hasMore`, or `undefined` when nothing in the response
   * spoke to it.
   *
   * `hasMore: false` conflates two different answers — "the server said this is
   * the last page" and "the server said nothing, so we stopped" — and only the
   * second one is worth telling the caller about. Several Oxy list endpoints
   * take `page`/`page_size` (or `limit`/`offset`) and answer with a BARE ARRAY:
   * they really are paginated, the response just carries no way to know it, so
   * `--paginate` returns page one and looks complete. Naming the rule is what
   * lets `paginate()` say so instead.
   */
  signal?: "next_offset" | "pagination.has_next" | "has_more" | "pagination.total_pages";
  /**
   * The `offset` the next request must carry, when `next_offset` decided.
   *
   * The other signals only say THAT there is another page, and the walk then
   * asks for `?page=<n+1>`. An endpoint that pages by `limit`/`offset` ignores
   * `page`, so for it the offset is the only way to reach the next rows.
   */
  nextOffset?: number;
}

/** A JSON object, narrowed enough to index. */
type JsonObject = Record<string, unknown>;

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Read one page: where the rows are, and whether to ask for another.
 *
 * `paginateKey` wins outright. Otherwise the rows are the first array-valued
 * field — first rather than largest, because a response with two arrays
 * (`{threads:[…], tags:[…]}`) has one that is the page and one that is
 * metadata, and declaration order puts the page first in every handler here.
 */
export function readPage(payload: unknown, page: number, explicitKey?: string): PageShape {
  if (Array.isArray(payload)) {
    // A bare array carries NO "is there more" signal, so it is one page.
    //
    // The tempting reading — "a full page means ask for another" — is a bug,
    // not a heuristic: an endpoint that ignores `?page` returns the SAME array
    // every time, so that rule loops to the page cap and merges a hundred
    // copies of the same rows. A hundred duplicates presented as a result is
    // far worse than a missed page, which at least shows up as a short list.
    // This is the module's stated policy applied consistently: nothing
    // recognised means one page.
    return { rows: payload, hasMore: false };
  }
  if (!isObject(payload)) return { rows: [], hasMore: false };

  const rowsKey = explicitKey ?? Object.keys(payload).find((k) => Array.isArray(payload[k]));
  const rows = rowsKey && Array.isArray(payload[rowsKey]) ? (payload[rowsKey] as unknown[]) : [];

  const pagination = isObject(payload.pagination) ? payload.pagination : undefined;
  let hasMore = false;
  let signal: PageShape["signal"];
  // `next_offset` first: it is the one signal that also says WHERE the next
  // page starts. `null` is the server saying this is the last page — a
  // recognised answer, so it sets `signal` and does not trip the
  // "no pagination signal" warning.
  if (typeof payload.next_offset === "number") {
    return { rowsKey, rows, hasMore: true, signal: "next_offset", nextOffset: payload.next_offset };
  }
  if (payload.next_offset === null) {
    return { rowsKey, rows, hasMore: false, signal: "next_offset" };
  }
  if (pagination && typeof pagination.has_next === "boolean") {
    hasMore = pagination.has_next;
    signal = "pagination.has_next";
  } else if (typeof payload.has_more === "boolean") {
    hasMore = payload.has_more;
    signal = "has_more";
  } else if (pagination && typeof pagination.total_pages === "number") {
    hasMore = page < pagination.total_pages;
    signal = "pagination.total_pages";
  }

  return { rowsKey, rows, hasMore, signal };
}

/** Follow `Link: rel="next"` when the server bothers to send one. */
export function linkNext(headers: Record<string, string>): string | undefined {
  const link = headers.link ?? headers.Link;
  if (!link) return undefined;
  for (const part of link.split(",")) {
    const match = /<([^>]+)>\s*;\s*rel="?next"?/.exec(part.trim());
    if (match) return match[1];
  }
  return undefined;
}

/**
 * Whether the response spoke about pagination AT ALL.
 *
 * Deliberately not `linkNext(...) !== undefined`. The last page of a paginated
 * endpoint has no `rel="next"` — that is what makes it the last page — so
 * testing for one conflates "I am at the end of a collection that pages" with
 * "this endpoint has never heard of pagination", and the warning at the bottom
 * of `paginate()` would fire on every single-page result from an endpoint that
 * is doing everything right. Oxy's handlers emit `rel="first"` on every page
 * precisely so this question has an answer (`oxy_app_core::pagination`).
 */
export function hasLinkHeader(headers: Record<string, string>): boolean {
  return Boolean(headers.link ?? headers.Link);
}

/** Replace or add `?page=` on a path. */
export function withPage(path: string, page: number): string {
  const [base, query = ""] = path.split("?", 2);
  const params = new URLSearchParams(query);
  params.set("page", String(page));
  return `${base}?${params.toString()}`;
}

/** Replace or add `?offset=` on a path, keeping every other parameter (`limit` included). */
export function withOffset(path: string, offset: number): string {
  const [base, query = ""] = path.split("?", 2);
  const params = new URLSearchParams(query);
  params.set("offset", String(offset));
  return `${base}?${params.toString()}`;
}

/** The `?offset=` a path already carries, or 0. */
function offsetOf(path: string): number {
  const offset = Number(new URLSearchParams(path.split("?", 2)[1] ?? "").get("offset"));
  return Number.isFinite(offset) ? offset : 0;
}

/**
 * Walk every page and return one body.
 *
 * Merged (the default), the result is the LAST page's object with the rows
 * replaced by every row from every page — so `pagination.total` still reads
 * correctly and a `--jq '.threads[]'` written against a single page keeps
 * working unchanged. Slurped (`--slurp`), it is an array of whole pages,
 * which is what you want when the per-page metadata is the point.
 */
export async function paginate(opts: PaginateOptions): Promise<string> {
  const method = opts.method.toUpperCase();
  if (method !== "GET") {
    // gh refuses this too. A paginated POST would re-submit the body once per
    // page, which for anything non-idempotent is a very expensive surprise.
    throw new CliError(`--paginate cannot be used with ${method}`, {
      code: ExitCode.USAGE,
      hint: "it would repeat the request body once per page"
    });
  }

  const limit = opts.maxPages ?? MAX_PAGES;
  const pages: unknown[] = [];
  const merged: unknown[] = [];
  let rowsKey: string | undefined;
  let lastPayload: unknown;
  let path = opts.path;
  let page = 1;
  /** Whether ANY page carried a signal this module knows how to read. */
  let sawSignal = false;

  for (; page <= limit; page++) {
    const response: ApiResponse = await request({ ...opts, path });
    if (response.status < 200 || response.status >= 300) throw errorForResponse(response);

    const payload = parseJson(response.body);
    if (payload === undefined) {
      // Not JSON — there is nothing to merge, so one page is the answer and
      // the raw body is it.
      return response.body;
    }
    lastPayload = payload;
    pages.push(payload);

    const shape = readPage(payload, page, opts.paginateKey);
    rowsKey ??= shape.rowsKey;
    merged.push(...shape.rows);

    // ANY `Link` counts, not only one naming a next page — a last page is still
    // a page of something that paginates, and saying otherwise is what made this
    // warning fire on the endpoints that answer correctly.
    sawSignal ||= hasLinkHeader(response.headers) || shape.signal !== undefined;

    const next = linkNext(response.headers);
    if (next) {
      path = next.startsWith("http") ? new URL(next).pathname + new URL(next).search : next;
      continue;
    }
    if (!shape.hasMore) break;
    if (shape.nextOffset === undefined) {
      path = withPage(opts.path, page + 1);
      continue;
    }
    // An offset that does not advance would fetch this same page until the
    // cap and merge every copy. Stopping is the visible failure; said out
    // loud because what was merged so far is not the whole collection.
    if (shape.nextOffset <= offsetOf(path)) {
      log.warn(
        `${opts.path} answered next_offset ${shape.nextOffset}, which is not past the current offset — stopped; the result is INCOMPLETE.`
      );
      break;
    }
    path = withOffset(path, shape.nextOffset);
  }

  if (page > limit) {
    log.warn(
      `stopped after ${limit} pages — the result is TRUNCATED. Narrow the query, or raise --max-pages.`
    );
  }

  // NOTHING RECOGNISED, SO ONE PAGE — said out loud rather than implied by a
  // short result. The heuristic stopping is the safe behaviour and stays; what
  // was missing is that the caller could not tell it apart from an endpoint
  // that genuinely had one page. Much of the admin surface takes `page` /
  // `page_size` and answers with a bare array, so `--paginate` there returns
  // the first 50 rows and reads as the whole table.
  //
  // Only on the opt-in flag, and only on stderr: someone who typed --paginate
  // asked to walk the pages, so "I could not" is an answer to their question,
  // not noise. It never fires for an endpoint that reported `has_next: false`.
  if (!sawSignal) {
    log.warn(
      `${opts.path} returned no pagination signal — this is ONE page, not necessarily every row.`
    );
    log.hint(
      "if the endpoint takes page/page_size or limit/offset, walk it yourself: `oxyc api '<path>?page=1'`"
    );
  }

  if (opts.slurp) return JSON.stringify(pages, null, 2);
  if (rowsKey && isObject(lastPayload)) {
    return JSON.stringify({ ...lastPayload, [rowsKey]: merged }, null, 2);
  }
  return JSON.stringify(merged, null, 2);
}
