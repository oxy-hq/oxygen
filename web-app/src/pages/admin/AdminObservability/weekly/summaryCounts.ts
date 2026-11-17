import type { UsageSummary } from "@/types/usageReport";
import { formatBytes, formatCount, plural } from "./utils";

/**
 * The quiet line of counts under the headline: each one a figure and the words after it.
 *
 * The first three are the report's own denominators and are always there — "0 of 12 apps
 * in use" is the finding, not an absence. The rest describe things that may simply not
 * have happened, and a line reading "0 function calls, 0 releases" is three facts' worth
 * of ink saying nothing, so each of those is left out when there is nothing to count.
 */
export type SummaryCountId = "views" | "apps" | "orgs" | "functions" | "releases" | "storage";

export interface SummaryCount {
  id: SummaryCountId;
  /** The figure, shown in the foreground colour. */
  value: string;
  /** The words after it, muted. */
  label: string;
}

export function summaryCounts(summary: UsageSummary): SummaryCount[] {
  const counts: SummaryCount[] = [
    {
      id: "views",
      value: formatCount(summary.views),
      label: plural(summary.views, "app open", "app opens")
    },
    {
      id: "apps",
      value: `${formatCount(summary.active_apps)} of ${formatCount(summary.apps)}`,
      label: `${plural(summary.apps, "app", "apps")} in use`
    },
    {
      id: "orgs",
      value: `${formatCount(summary.active_orgs)} of ${formatCount(summary.orgs)}`,
      label: `${plural(summary.orgs, "organization", "organizations")} active`
    }
  ];

  const calls = summary.function_calls;
  const failed = summary.function_failures;
  // A failure keeps the item on the line even beside a call count of zero. The two
  // should never disagree like that, and if they do, the failure is the part to show.
  if (calls > 0 || failed > 0) {
    const noun = plural(calls, "function call", "function calls");
    counts.push({
      id: "functions",
      value: formatCount(calls),
      label: failed > 0 ? `${noun}, ${formatCount(failed)} failed` : noun
    });
  }

  if (summary.releases > 0) {
    counts.push({
      id: "releases",
      value: formatCount(summary.releases),
      label: plural(summary.releases, "release", "releases")
    });
  }

  // `null` is "nothing was measured". Zero is left out with it: "0 B stored" across the
  // whole report says no more than the line's silence does.
  if (summary.storage_bytes !== null && summary.storage_bytes > 0) {
    counts.push({ id: "storage", value: formatBytes(summary.storage_bytes), label: "stored" });
  }

  return counts;
}
