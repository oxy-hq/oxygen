/**
 * Anomaly Inbox types — mirror the Rust `entity::metric_anomalies::Model`
 * and the `metric_anomalies` HTTP responses.
 */

export type AnomalySeverity = "low" | "medium" | "high";
export type AnomalyStatus = "new" | "acknowledged" | "dismissed";

/** A single dimension filter identifying which segment an anomaly belongs to. */
export interface AnomalyFilter {
  /** Fully-qualified dimension id, e.g. `"labor_daily.restaurant_id"`. */
  member: string;
  /** Matched values (OR within a filter). */
  values: string[];
}

export interface MetricAnomaly {
  id: string;
  workspace_id: string;
  measure: string;
  time_dimension: string;
  granularity: string;
  period_start: string;
  period_end: string;
  observed: number;
  expected: number;
  lower_bound: number;
  upper_bound: number;
  z_score: number;
  severity: AnomalySeverity;
  status: AnomalyStatus;
  label: string | null;
  /**
   * Stable key derived from the monitor's filters (e.g.
   * `"labor_daily.restaurant_id=loc-abc"`). Empty string for chain-wide
   * (unfiltered) monitors. Distinguishes per-segment anomalies that share a
   * measure/period so they don't read as duplicates.
   */
  dimension_key: string;
  /** Raw filters identifying this anomaly's segment. Null for chain-wide monitors. */
  filters: AnomalyFilter[] | null;
  /**
   * Groups consecutive flagged buckets of one segment into a single event, so a
   * surge spanning Mon/Wed/Thu reads as one problem rather than three. Rows stay
   * per-bucket (explain reasons about a single bucket), so the collapsing
   * happens here on read. Null for rows detected before events existed.
   */
  event_id: string | null;
  detected_at: string;
  updated_at: string;
}

export interface ListAnomaliesResponse {
  anomalies: MetricAnomaly[];
  /** Total across every page — **events** under the default ranking (the same
   *  unit as `limit`/`offset`, so `total / limit` is the page count), rows
   *  under `order=recent`.
   *
   *  Absent when the request sent neither `limit` nor `offset`: that asks for
   *  "the top N", and there is no total behind it.
   *
   *  Also absent when the count query failed — the server serves the page it
   *  already has rather than failing over a denominator. That case is why the
   *  inbox carries an uncounted pager at all, so read `undefined` as "unknown",
   *  never as zero. */
  total?: number;
  /** The page the server actually served — `limit` is clamped to 1..=500, so a
   *  client that pages must read it back rather than trust what it asked for.
   *
   *  Optional because a replica still running a pre-paging build sends neither,
   *  which is a live shape during a rolling deploy — the inbox guards on that,
   *  and typing these as required would let a refactor delete the guard and
   *  reintroduce a `?offset=NaN` request. */
  limit?: number;
  offset?: number;
  /** The deepest `offset` the server will serve — past it a request is a 400.
   *  Echoed so the pager can stop offering pages that don't exist, rather than
   *  the client keeping its own copy of the number and drifting from it. */
  max_offset?: number;
  /** Event keys whose buckets were trimmed to the server's per-event cap, in
   *  the same key space `groupIntoEvents` builds (`event_id`, or
   *  `ungrouped:<row id>`). Without it a row cannot tell a complete 50-bucket
   *  event from a trimmed 200-bucket one, and "worst of N" would be a guess.
   *
   *  Only populated under the default ranking, which pages whole events. */
  truncated_events?: string[];
}

/** One status write: which anomalies, and the status they were shown as.
 *  Built by the inbox's `targetOf`; consumed by the update hook and service. */
export interface StatusWriteGroup {
  /** Only buckets already in one of these statuses are written.
   *
   *  A set, not the tab's single status. An event can hold buckets the user
   *  dismissed weeks ago and can't see from here — acking the row must not
   *  resurrect those — while a scan can chain a fresh `new` bucket onto an
   *  already-acknowledged event, which a single-status bound would strand.
   *  So: the live statuses, plus `dismissed` when the row itself is dismissed. */
  onlyStatuses: AnomalyStatus[];
  ids: string[];
  eventIds: string[];
}

export interface BulkUpdateStatusResponse {
  /** Buckets the server actually wrote — lower than what was asked for when a
   *  row was deleted, moved, or belongs to another workspace. */
  updated: number;
  /** Distinct anomalies (events, or standalone rows) behind those buckets —
   *  the unit the UI counts in, so a partial apply can be reported honestly.
   *
   *  An anomaly counts once any of its buckets is written, which means what it
   *  says only when the write named the event. `targetOf` uses bare row ids
   *  solely for pre-event rows, which are one bucket each. */
  events_updated: number;
}

/** A monitor that errored during a scan — identifies the monitor/segment and the error. */
export interface ScanFailure {
  measure: string;
  time_dimension: string;
  granularity: string;
  label: string | null;
  /** Segment key when the failed monitor was a group_by/filtered segment; empty for chain-wide. */
  dimension_key: string;
  /** Raw filters identifying the segment; null for chain-wide monitors. */
  filters: AnomalyFilter[] | null;
  error: string;
}

export interface ScanAnomaliesResponse {
  monitors_scanned: number;
  monitors_failed: number;
  anomalies_persisted: number;
  /** True when the scan is still running in the background. Refetch anomalies after a short delay. */
  pending?: boolean;
  /** Per-monitor failures. Empty on a clean scan and on the `pending` path. */
  failures?: ScanFailure[];
}

export interface MonitorEntry {
  measure: string;
  time_dimension: string;
  granularity: "day" | "week" | "month";
  lookback_days: number;
  seasonality: number[] | null;
  sensitivity: "low" | "medium" | "high";
  label?: string | null;
  /**
   * Dimension filters narrowing this entry to one segment. Two entries over the
   * same measure/time-dimension/granularity are distinguished only by these, so
   * anything mapping coverage rows back to an entry must use them.
   */
  filters?: AnomalyFilter[];
  /**
   * Fan-out dimension: the scanner discovers its values at scan time and files
   * one coverage row per segment, each keyed by this entry's `filters` *plus*
   * the discovered value.
   */
  group_by?: string | null;
}

/** Per-segment scan coverage. A `group_by` monitor fans out to one row per
 *  segment, so a single `MonitorEntry` can map to many of these.
 *
 *  Exists because a monitor skipped for want of history produces neither an
 *  anomaly nor a failure — without this the UI cannot tell "healthy, nothing
 *  found" from "not scoring at all". */
export interface MonitorCoverage {
  id: string;
  workspace_id: string;
  measure: string;
  time_dimension: string;
  granularity: string;
  /** Empty string for chain-wide monitors. */
  dimension_key: string;
  filters: Record<string, unknown>[] | null;
  label: string | null;
  /** Buckets the warehouse returned; zero-filled gaps are not counted. */
  measured_buckets: number;
  /** The statistical floor this segment must clear to be scored. */
  required_buckets: number;
  last_scanned_at: string;
}

/** The `notify:` block of `.monitor.yml`: where a scan posts the insights it
 *  newly found, and from what severity. */
export interface MonitorNotify {
  /** A Slack channel id in the org's connected Slack, e.g. `C0123ABCDEF`. */
  slack_channel: string;
  min_severity: AnomalySeverity;
}

export interface ListMonitorsResponse {
  monitors: MonitorEntry[];
  coverage: MonitorCoverage[];
  /** Absent when the file has no `notify:` block. */
  notify?: MonitorNotify | null;
}
