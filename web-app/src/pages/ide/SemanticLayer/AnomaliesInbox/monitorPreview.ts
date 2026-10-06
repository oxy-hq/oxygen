import type { MonitorEntry, MonitorPreview, PreviewFlag } from "@/types/metricAnomalies";

/** "the 7 most recent days", "the most recent week" — the buckets a scan of
 *  this grain scores.
 *
 *  "Most recent", not "last 7 days": a scan scores the newest buckets the
 *  series *has*. On a measure whose data stopped in July, that is a week in
 *  July, and calling it "the last 7 days" in October would be false. */
export function windowPhrase(granularity: MonitorEntry["granularity"], buckets: number): string {
  return buckets === 1
    ? `the most recent ${granularity}`
    : `the ${buckets} most recent ${granularity}s`;
}

export type PreviewTone = "flagged" | "quiet" | "warming" | "failed";

/** How many segments came out each way. */
export function tally(preview: MonitorPreview) {
  const counts = { flagging: 0, quiet: 0, warming: 0, failed: 0 };
  for (const s of preview.segments) {
    if (s.state === "failed") counts.failed += 1;
    else if (s.state === "warming_up") counts.warming += 1;
    else if (s.flagged.length > 0) counts.flagging += 1;
    else counts.quiet += 1;
  }
  return counts;
}

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

/**
 * The one sentence a preview comes down to, and what kind of answer it is.
 *
 * The order of the checks is the order of what an author most needs to know:
 * a monitor that could not run says nothing about its data, and one that is
 * not being scored has not been checked — neither may read as "nothing
 * found". Only a monitor that was scored and flagged nothing gets to say so.
 */
export function previewHeadline(
  preview: MonitorPreview,
  granularity: MonitorEntry["granularity"]
): { tone: PreviewTone; text: string } {
  const window = windowPhrase(granularity, preview.window_buckets);
  const counts = tally(preview);

  // A `group_by` whose dimension has no values. A scan skips such a monitor;
  // "across 0 segments, nothing would be flagged" would call that healthy.
  if (preview.segments.length === 0) {
    return {
      tone: "warming",
      text: "Nothing to score — this monitor's group_by dimension has no values in its lookback window."
    };
  }

  if (preview.segments.length === 1) {
    const only = preview.segments[0];
    if (only.state === "failed") return { tone: "failed", text: "This monitor could not be run." };
    if (only.state === "warming_up") {
      return {
        tone: "warming",
        text: `Not scored yet — ${only.measured_buckets} of the ${plural(only.required_buckets, granularity)} of history it needs.`
      };
    }
    if (only.flagged.length === 0) {
      return { tone: "quiet", text: `A scan now would flag nothing in ${window}.` };
    }
    return {
      tone: "flagged",
      text:
        preview.window_buckets === 1
          ? `A scan now would flag ${window}.`
          : `A scan now would flag ${only.flagged.length} of ${window}.`
    };
  }

  const parts: string[] = [];
  if (counts.flagging > 0) parts.push(`${counts.flagging} would be flagged`);
  if (counts.warming > 0) parts.push(`${counts.warming} not scored yet`);
  if (counts.failed > 0) parts.push(`${counts.failed} could not be run`);
  const scope =
    preview.segments_total > preview.segments.length
      ? `the first ${preview.segments.length} of ${plural(preview.segments_total, "segment")}`
      : plural(preview.segments.length, "segment");
  // Red only when nothing ran at all. One segment in a dozen failing is
  // something the sentence says and that segment's own block shows; painting
  // the whole answer as a failure would bury the eleven that have one.
  const tone: PreviewTone =
    counts.failed === preview.segments.length
      ? "failed"
      : counts.flagging > 0
        ? "flagged"
        : counts.warming > 0 || counts.failed > 0
          ? "warming"
          : "quiet";
  return {
    tone,
    text:
      parts.length === 0
        ? `Across ${scope}, a scan now would flag nothing in ${window}.`
        : `Across ${scope}: ${parts.join(", ")}.`
  };
}

/** `(observed − expected) / |expected|` as a percentage — the Insights
 *  Inbox's own Δ%, and `null` on the same condition it shows a dash for. */
export function deviationPercent(flag: PreviewFlag): number | null {
  if (Math.abs(flag.expected) < 1e-9) return null;
  return ((flag.observed - flag.expected) / Math.abs(flag.expected)) * 100;
}
