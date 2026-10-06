import type { AnomalyFilter, MonitorCoverage, MonitorEntry } from "@/types/metricAnomalies";

/** Mirror of `MonitorFilter::key_for` (crates/metric-monitoring/src/config.rs):
 *  `member=v1,v2` pairs with values sorted, pairs sorted, joined by `;`. This
 *  is the string the scanner stores as a coverage row's `dimension_key`. */
export function filterKey(filters: AnomalyFilter[] | null | undefined): string {
  if (!filters || filters.length === 0) return "";
  return filters
    .map((f) => `${f.member}=${[...f.values].sort().join(",")}`)
    .sort()
    .join(";");
}

/** The coverage rows belonging to one monitor entry, within rows already
 *  narrowed to its (measure, time_dimension, granularity) triple.
 *
 *  An entry without `group_by` has exactly one segment, so it matches its own
 *  filter key exactly — that is what keeps two entries differing only by
 *  `filters` (region=US vs region=EU) from rendering each other's segments. A
 *  `group_by` entry fans out at scan time, so its segments carry its filters
 *  *plus* the discovered value and are matched by containment instead. */
export function coverageFor(entry: MonitorEntry, rows: MonitorCoverage[]): MonitorCoverage[] {
  const own = filterKey(entry.filters);
  if (!entry.group_by) return rows.filter((c) => c.dimension_key === own);
  if (own === "") return rows;
  const required = own.split(";");
  return rows.filter((c) => {
    const pairs = new Set(c.dimension_key ? c.dimension_key.split(";") : []);
    return required.every((p) => pairs.has(p));
  });
}

/** How a sensitivity is badged. A setting, not a state: `high` is the monitor
 *  doing what its author asked, so nothing here is ever the error colour —
 *  red in this table would read as a monitor that is failing. */
export function sensitivityVariant(s: MonitorEntry["sensitivity"]): "secondary" | "outline" {
  return s === "high" ? "secondary" : "outline";
}

function bucketNoun(granularity: string, n: number): string {
  const base = granularity === "week" ? "week" : granularity === "month" ? "month" : "day";
  return n === 1 ? base : `${base}s`;
}

/** What to show in the Coverage column for one monitor's segments.
 *
 *  `null` means "nothing to say" — either the monitor is being scored normally
 *  or it has never been scanned. Only the warming-up case earns a badge, since
 *  that is the state an empty inbox would otherwise hide.
 *
 *  `detail` is a list of separate facts rather than one joined sentence, so
 *  the table can set them apart with space instead of punctuation. */
export function warmingSummary(
  rows: MonitorCoverage[]
): { label: string; detail: string[] } | null {
  const warming = rows.filter((c) => c.measured_buckets < c.required_buckets);
  if (warming.length === 0) return null;

  // A monitor without group_by has exactly one segment, so name the real
  // numbers rather than an unhelpful "1 of 1 segments".
  if (rows.length === 1) {
    const only = warming[0];
    return {
      label: "Warming up",
      detail: [
        `${only.measured_buckets} of ${only.required_buckets} ${bucketNoun(
          only.granularity,
          only.required_buckets
        )}`
      ]
    };
  }

  // Fanned out by group_by. The count alone hides how long the wait is, so
  // report the segment furthest from clearing the floor alongside it.
  const furthest = warming.reduce((a, b) =>
    a.required_buckets - a.measured_buckets >= b.required_buckets - b.measured_buckets ? a : b
  );
  return {
    label: warming.length === rows.length ? "Warming up" : "Partly warming up",
    detail: [
      `${warming.length} of ${rows.length} segments`,
      `furthest ${furthest.measured_buckets} of ${furthest.required_buckets}`
    ]
  };
}

export function relativeTime(isoDate: string): string {
  const days = Math.floor((Date.now() - new Date(isoDate).getTime()) / 86_400_000);
  if (days === 0) return "today";
  if (days === 1) return "1 day ago";
  return `${days} days ago`;
}
