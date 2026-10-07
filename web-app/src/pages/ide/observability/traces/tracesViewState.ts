/**
 * Where a Traces view *is*, expressed as query params.
 *
 * Same split as the admin app view (`appViewState.ts`): a **location** goes in
 * the URL, a personal preference does not. "Errors in the last 7 days matching
 * `revenue`, page 3" is a place someone sends a colleague, and the place Back
 * has to return to from a trace — while this lived in component state, opening
 * a trace and coming back reset every filter and dropped the reader on page 1.
 *
 * ## What is deliberately absent
 *
 * - Card-vs-table: how the reader likes to look at the list, not where they are.
 * - Live-tail: a mode of this sitting. A link that started polling in the
 *   recipient's tab would be doing something they did not ask for.
 * - The compare selection: it names rows of one page load, and a rolling window
 *   moves those rows between loads.
 */

import { DURATION_VALUES, type DurationValue, type StatusFilter, type TimeRange } from "./types";

export interface TracesViewState {
  timeRange: TimeRange;
  /** Trimmed free-text search; empty means no search. */
  search: string;
  status: StatusFilter;
  /** 1-based. */
  page: number;
}

const DEFAULT_DURATION: DurationValue = "30d";

const STATUSES: readonly StatusFilter[] = ["all", "ok", "error"];

/** Query-param names, in one place so the reader and the writer cannot drift. */
const PARAM = {
  duration: "range",
  from: "from",
  to: "to",
  search: "q",
  status: "status",
  page: "page"
} as const;

/** A whole, positive number, or `null` — `Number("")` is 0 and `Number("1e3")` is 1000. */
function positiveInt(raw: string | null): number | null {
  if (raw === null || !/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n > 0 ? n : null;
}

function readTimeRange(params: URLSearchParams): TimeRange {
  // An absolute range wins over a preset, matching the API (`from`/`to`
  // override `duration`). Half a range, or one that runs backwards, is not a
  // range: fall through rather than query a window nobody chose.
  const from = positiveInt(params.get(PARAM.from));
  const to = positiveInt(params.get(PARAM.to));
  if (from !== null && to !== null && from < to) return { kind: "custom", from, to };

  const duration = params.get(PARAM.duration);
  return {
    kind: "preset",
    value: DURATION_VALUES.includes(duration as DurationValue)
      ? (duration as DurationValue)
      : DEFAULT_DURATION
  };
}

/**
 * Read the view out of a query string.
 *
 * Every field validates against what the page can render and falls back rather
 * than throwing: a URL is user input, and a hand-edited `?status=failed` should
 * show every trace, not an error boundary.
 */
export function readTracesViewState(params: URLSearchParams): TracesViewState {
  const status = params.get(PARAM.status);
  return {
    timeRange: readTimeRange(params),
    search: params.get(PARAM.search)?.trim() ?? "",
    status: STATUSES.includes(status as StatusFilter) ? (status as StatusFilter) : "all",
    page: positiveInt(params.get(PARAM.page)) ?? 1
  };
}

/**
 * Apply a patch to an existing query string, dropping params that are back at
 * their default so the URL someone copies names only what they changed. Params
 * this module does not own are left alone.
 */
export function writeTracesViewState(
  current: URLSearchParams,
  patch: Partial<TracesViewState>
): URLSearchParams {
  const next = new URLSearchParams(current);
  const set = (key: string, value: string, isDefault: boolean) => {
    if (isDefault) next.delete(key);
    else next.set(key, value);
  };

  if (patch.timeRange) {
    // The two forms are mutually exclusive, so writing one clears the other.
    next.delete(PARAM.duration);
    next.delete(PARAM.from);
    next.delete(PARAM.to);
    if (patch.timeRange.kind === "custom") {
      next.set(PARAM.from, String(patch.timeRange.from));
      next.set(PARAM.to, String(patch.timeRange.to));
    } else {
      set(PARAM.duration, patch.timeRange.value, patch.timeRange.value === DEFAULT_DURATION);
    }
  }
  if (patch.search !== undefined) set(PARAM.search, patch.search, patch.search === "");
  if (patch.status !== undefined) set(PARAM.status, patch.status, patch.status === "all");
  if (patch.page !== undefined) set(PARAM.page, String(patch.page), patch.page <= 1);
  return next;
}
