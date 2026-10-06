import type { FunctionLogLine } from "@/types/apps";

/**
 * The windows the Logs section offers. The widest is the route's own ceiling
 * (`MAX_HOURS` in `custom_apps_logs.rs`); asking for more is clamped to it, so
 * a wider option here would be a label the answer does not match.
 */
export const LOG_WINDOWS = [
  { hours: 1, label: "1h", phrase: "the last hour" },
  { hours: 24, label: "24h", phrase: "the last 24 hours" },
  { hours: 168, label: "7d", phrase: "the last 7 days" }
] as const;

export type LogWindowHours = (typeof LOG_WINDOWS)[number]["hours"];

export const DEFAULT_LOG_WINDOW: LogWindowHours = 24;
export const WIDEST_LOG_WINDOW: LogWindowHours = 168;

export function windowPhrase(hours: LogWindowHours): string {
  return LOG_WINDOWS.find((w) => w.hours === hours)?.phrase ?? `the last ${hours} hours`;
}

/** Lines one read of a window asks for. */
export const WINDOW_LOG_LIMIT = 200;
/** Lines a read narrowed to one request asks for — the route's ceiling. */
export const REQUEST_LOG_LIMIT = 500;

const UUID = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i;

/** What the request-id box holds, read the way the route will read it. */
export type RequestFilter =
  | { kind: "none" }
  | { kind: "id"; requestId: string }
  | { kind: "invalid" };

/**
 * An `x-oxy-request-id` is a UUID, and the route refuses anything else with a
 * 400 instead of an empty list. So a half-pasted id is caught here: sending it
 * would turn "keep typing" into "could not read function logs".
 */
export function parseRequestFilter(raw: string): RequestFilter {
  const trimmed = raw.trim();
  if (trimmed === "") return { kind: "none" };
  return UUID.test(trimmed)
    ? { kind: "id", requestId: trimmed.toLowerCase() }
    : { kind: "invalid" };
}

/** One invocation's output: the unit an operator reads, and the unit ids name. */
export interface InvocationLogs {
  key: string;
  /** The earliest line held. Function, mode and ids are the same on every line. */
  head: FunctionLogLine;
  hasError: boolean;
  /** In the order they were written. */
  lines: FunctionLogLine[];
}

/**
 * Regroup a newest-first page of lines by invocation.
 *
 * The flat list interleaves concurrent invocations and reads bottom-up, which
 * is the wrong way round for "what did this call do". Groups keep the page's
 * order — the invocation with the newest line first — and each one reads top
 * to bottom.
 */
export function groupByInvocation(lines: FunctionLogLine[]): InvocationLogs[] {
  const groups = new Map<string, FunctionLogLine[]>();
  lines.forEach((line, index) => {
    // A line with no invocation id has nothing to be grouped by; it stands alone.
    const key = line.invocation_id || `line-${index}`;
    const group = groups.get(key);
    if (group) group.push(line);
    else groups.set(key, [line]);
  });
  return [...groups].map(([key, group]) => {
    const ordered = [...group].sort((a, b) => a.seq - b.seq);
    return {
      key,
      head: ordered[0],
      hasError: ordered.some((line) => line.level === "error"),
      lines: ordered
    };
  });
}

/**
 * A line's time as the wire carries it: UTC, to the second. The date joins it
 * once the window is wider than a day, where `14:02:11` alone names seven
 * different moments.
 */
export function formatLogTime(iso: string, withDate: boolean): string {
  const time = iso.slice(11, 19);
  if (!time) return iso;
  return withDate ? `${iso.slice(5, 10)} ${time}` : time;
}
