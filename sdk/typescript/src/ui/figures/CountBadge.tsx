// New as a component: no app has one by name. Started from the count the reference draws inline
// in its tabs and segments (oxy-hq/reference-app@3caad1f `src/components/DefaultTabs.tsx:246-256`,
// the `tabCount` recipe, `ui.ts:907-921`), so a count beside a heading, a rail item or a filter
// is the same mark as the one in a tab. Square, by decision 38b ("count badges, square"): the
// corner is the theme's `--badge-radius`, 0 in the reference.
//
// **The tone is the worst status the count holds**, the caller's to work out with
// `strongestTone(...)` over the records it counts (error over warning over everything else);
// left out, the count is neutral, which is the answer for most counts. A count is not a verdict
// on its own, so success and info are for an outcome summary that means them.
//
// **`selected`**: a neutral count on a selected segment or tab takes the selected ground's own
// neutral (`tabCountNeutralOn`), as DefaultTabs does. A toned count keeps its tone there.
//
// **Read as words.** `label` ("3 overdue") replaces the bare figure for a screen reader, which
// otherwise hears "3" with nothing to say what of. `max` caps the figure ("99+"); the label, or
// the full count, is still what is read.

import type { ComponentPropsWithRef } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";

export interface CountBadgeProps extends Omit<ComponentPropsWithRef<"span">, "children"> {
  count: number;
  /** The worst status the count holds (`strongestTone`). Neutral when left out. */
  tone?: StatusTone;
  /** It sits on a selected segment or tab: a neutral count takes that ground's neutral. */
  selected?: boolean;
  /** The largest figure drawn; above it the badge reads `${max}+`. */
  max?: number;
  /** What a screen reader hears in place of the figure: "3 overdue". */
  label?: string;
}

/** The figure a badge draws: the count, or `${max}+` above `max`. */
export function countBadgeText(count: number, max?: number): string {
  if (max !== undefined && count > max) return `${max}+`;
  return String(count);
}

export function CountBadge({
  count,
  tone,
  selected = false,
  max,
  label,
  className,
  ...rest
}: CountBadgeProps) {
  const neutral = tone === undefined || tone === "neutral";
  const shown = countBadgeText(count, max);
  const spoken = label ?? (shown === String(count) ? undefined : String(count));
  return (
    <span
      {...rest}
      className={cx(
        ui.tabCount,
        "oxy-ui-count-badge",
        neutral ? (selected ? ui.tabCountNeutralOn : ui.tabCountNeutral) : ui.chipTone[tone],
        className
      )}
      data-tone={neutral ? "neutral" : tone}
    >
      {spoken === undefined ? (
        shown
      ) : (
        <>
          <span aria-hidden='true'>{shown}</span>
          <span className='oxy-ui-count-badge__label'>{spoken}</span>
        </>
      )}
    </span>
  );
}
