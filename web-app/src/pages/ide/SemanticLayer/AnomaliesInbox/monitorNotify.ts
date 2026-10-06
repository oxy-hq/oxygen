import type { MonitorNotify } from "@/types/metricAnomalies";

/**
 * Which insights the file's `notify:` block announces, as the words that
 * follow "New insights".
 *
 * `min_severity` is a floor, so `medium` has to read as "medium or above" —
 * "at medium severity" would say a high one is left out.
 */
export function announcedSeverity(notify: MonitorNotify): string {
  if (notify.min_severity === "high") return "at high severity";
  if (notify.min_severity === "medium") return "at medium severity or above";
  return "of any severity";
}
