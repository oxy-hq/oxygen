// Started from oxy-hq/reference-app@3caad1f `src/components/MetricCell.tsx` (INVENTORY section 3:
// the reference over Watchlist's, its one copy, which has a state fewer). Kept: one line and one
// number, the figure and its comparison as one unbreakable run, right-set so a column is read down
// its right edge, the five looks that work without hue, the "as of" note, and a sentence for every
// state. New: what the reference worked out from its own roll-up (`cellText(cell, metric,
// periodId, thresholds)`, `lib/storeRollup.ts`, `lib/severity.ts`) comes in as props, so no
// measure, store or threshold of any app lives here: the figures arrive formatted, and the step is
// given or judged from `value` against `thresholds`. 2.31.0: the `quiet` step.
//
// One record's one measure, in a table cell. Six looks, ordered by how much attention each asks:
//
//   tint + status ink   `warning` / `error`: flagged, semibold. The only background in the
//                       table, so the only thing that interrupts a scan.
//   tint, regular       `quiet`: the quieter warning step, the warning tint and ink at weight 400
//                       (decision 93, decision 7 reread): an app with two warning tiers draws the
//                       lower one so (Watchlist: notable -> `quiet`, elevated -> `warning`, severe
//                       -> `error`). Weight is its only difference from `warning`.
//   semibold, full ink  `notable`: worth a glance. Weight, not fill.
//   regular, dim ink    `clear`: checked and fine, the plain default most figures wear.
//   subtle + asterisk   `provisional`: shown but not judged (too few to compare against).
//   subtle words        `label` alone: nothing to judge.
//
// The comparison ("vs 17.4%") shows only where the cell asserts something or refuses to (a flag,
// a provisional figure); a clear cell's comparison is in its sentence. Every look says the whole
// thing in a sentence: `hint`, in a HoverHint (hover, or a tap on a phone) and, since a plain
// cell is no focus stop, as words a screen reader reads after the figure.

import type { ReactNode } from "react";
// The containers group's HoverHint (the reference's MetricCell wraps itself in the same), read
// from its file until containers/index.ts exports it.
import { HoverHint } from "../containers";
import { cx } from "../cx";
import { ui } from "../recipes";

/** How loudly a figure is marked. */
export type MetricStep = "clear" | "notable" | "quiet" | "warning" | "error";

/**
 * Where a figure's steps begin, on the app's own scale (a percentage, a multiple of a bar, a
 * rating). A figure AT a threshold has reached it.
 */
export interface MetricThresholds {
  /** Worth a glance: semibold, no fill. Omit for a measure with two flagged steps. */
  notable?: number;
  /** The quieter warning step: the warning tint at weight 400. Omit for one warning step. */
  quiet?: number;
  warning: number;
  /** Omit when nothing on this measure is ever an error. */
  error?: number;
  /** `above` (the default): bigger is worse, a cost. `below`: smaller is worse, a rating. */
  direction?: "above" | "below";
}

/** The step `value` has reached, or null when there is no value to judge. */
export function metricStep(
  value: number | null | undefined,
  thresholds: MetricThresholds
): MetricStep | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null;
  const { notable, quiet, warning, error, direction = "above" } = thresholds;
  const reached = (bar: number | undefined) =>
    bar !== undefined && (direction === "above" ? value >= bar : value <= bar);
  if (reached(error)) return "error";
  if (reached(warning)) return "warning";
  if (reached(quiet)) return "quiet";
  if (reached(notable)) return "notable";
  return "clear";
}

export interface MetricCellProps {
  /** The figure, formatted by the app ("24.0%"). Absent: the cell is `label` alone. */
  observed?: ReactNode;
  /** The number `thresholds` judge (often the gap, not the figure shown). */
  value?: number | null;
  thresholds?: MetricThresholds;
  /** The step, when the app has judged it already. Wins over `value` and `thresholds`. */
  step?: MetricStep | null;
  /** What the figure is measured against, formatted ("17.4%"). */
  comparison?: ReactNode;
  /** Print the comparison. Default: where the cell asserts something (a flag at any step) or
   *  refuses to (`provisional`); a clear cell keeps it in its sentence. */
  showComparison?: boolean;
  /** Shown but not judged: too thin to act on. Subtle ink and an asterisk; never tinted. */
  provisional?: boolean;
  /** The asterisk's title. */
  provisionalNote?: string;
  /** Words: the whole cell when there is no figure ("No books connected"), else a caption beside
   *  it ("2nd highest of 9"). */
  label?: ReactNode;
  /** The whole state in a sentence: in a HoverHint, and to a screen reader. */
  hint?: string;
  /** Set only when this record's period differs from the column's ("Jul 2026"). */
  asOf?: ReactNode;
  className?: string;
}

const FLAGGED: ReadonlySet<MetricStep> = new Set(["notable", "quiet", "warning", "error"]);

export function MetricCell({
  observed,
  value,
  thresholds,
  step: given,
  comparison,
  showComparison,
  provisional = false,
  provisionalNote = "Too few to compare against",
  label,
  hint,
  asOf,
  className
}: MetricCellProps) {
  const judged = given ?? (thresholds ? metricStep(value, thresholds) : null);
  // A provisional figure is never judged: it is marked as such and nothing else.
  const step: MetricStep | null = provisional ? null : judged;
  const has = (n: ReactNode) => n !== undefined && n !== null && n !== false && n !== "";
  const figure = has(observed);
  const compare =
    has(comparison) && (showComparison ?? ((step !== null && FLAGGED.has(step)) || provisional));
  // The quiet step wears the warning tone: the same tint and ink, at weight 400.
  const tone = step === "quiet" ? "warning" : step === "warning" || step === "error" ? step : null;
  const body = (
    <span
      className={cx("oxy-ui-metric-cell", className)}
      data-step={step ?? (provisional ? "provisional" : figure ? "clear" : "empty")}
    >
      {figure && (
        // The figure and what it is compared against are ONE unbreakable thing: as two flex
        // children they wrap apart, and the table's auto layout then starves the last columns.
        <span className='oxy-ui-metric-cell__run'>
          <span
            className={cx(
              "oxy-ui-metric-cell__figure",
              tone && ui.chipTone[tone],
              tone && "oxy-ui-metric-cell__figure--tinted",
              step === "notable" && "oxy-ui-metric-cell__figure--notable",
              step === "quiet" && "oxy-ui-metric-cell__figure--quiet",
              provisional && "oxy-ui-metric-cell__figure--provisional"
            )}
          >
            {observed}
          </span>
          {compare && (
            <span className='oxy-ui-metric-cell__comparison'>
              vs {comparison}
              {provisional && <span title={provisionalNote}>*</span>}
            </span>
          )}
        </span>
      )}
      {has(label) && (
        <span
          className={cx("oxy-ui-metric-cell__label", figure && "oxy-ui-metric-cell__label--beside")}
        >
          {label}
        </span>
      )}
      {has(asOf) && <span className='oxy-ui-metric-cell__as-of'>as of {asOf}</span>}
      {hint && <span className='oxy-ui-metric-cell__hint'>{hint}</span>}
    </span>
  );
  return hint ? (
    <HoverHint text={hint} className='oxy-ui-metric-cell__frame'>
      {body}
    </HoverHint>
  ) : (
    body
  );
}
