// Shared UI state types for the Traces surface (Theme 3).

/** Status filter in the toolbar. Maps to the API `status` query param. */
export type StatusFilter = "all" | "ok" | "error";

/** List rendering mode: rich cards or a dense table. */
export type TraceView = "card" | "table";

/** Rolling windows the time-range control offers. */
export const DURATION_VALUES = ["1h", "24h", "7d", "30d", "90d"] as const;
export type DurationValue = (typeof DURATION_VALUES)[number];

/** Either a rolling preset window or an absolute range (epoch seconds). */
export type TimeRange =
  | { kind: "preset"; value: DurationValue }
  | { kind: "custom"; from: number; to: number };

/** UI status → the `status_code` value the backend filters on (undefined = no filter). */
export function statusFilterToApi(status: StatusFilter): string | undefined {
  if (status === "ok") return "Ok";
  if (status === "error") return "Error";
  return undefined;
}
