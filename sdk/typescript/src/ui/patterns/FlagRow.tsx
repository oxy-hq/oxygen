// Started from oxy-hq/reference-app@81ee8aa `src/components/FlagRow.tsx` (INVENTORY section 4: one
// copy, Watchlist's `apps/watchlist/src/components/FlagRow.tsx`, which the reference grew from,
// diff 114; Locations' `Cases.tsx:102` cites it). Kept: five columns read left to right in the
// order a person asks them (rank; name and measure; the evidence sentence; the sparkline; the
// figure and its deviation), the rank at the subtle ink, the name in a fixed column from 48rem up
// so the measure chips line up, the measure as a filled neutral caps chip (it says WHICH measure
// tripped, not how badly), the sentence capped at the reading measure, the sparkline only from
// 40rem up, the figure as the row's headline, the whole row one button that opens the detail
// (`aria-haspopup="dialog"`, `aria-current` while it is the open one), and selection outranking
// hover. New, as a layout with slots: every cell is the caller's node (the sparkline is an SDK
// `Sparkline`, the deviation a `Chip` in the row's tone, each with the caller's words), and
// `actions`, a slot BESIDE the row's button (a button inside a button is not allowed), so a row
// can carry its own controls. Without `onSelect` the row is static: no button, no hover. The
// name and the figure are the reference's own `text-base font-medium` and `text-xl
// font-semibold`, on the scale's line (1.4), not the `ui.text.base` / `ui.text.xl` recipes (1.375,
// 1.25), which set the row 3.6px short (SDK-GAPS 5).
//
// A row of a ranked list: render it inside a `<ul>`.

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface FlagRowProps {
  /** Where the row sits in the queue: a number a reader may say out loud. */
  rank?: ReactNode;
  /** Who: the row's name. */
  name: ReactNode;
  /** What about them: the measure, drawn as a neutral caps chip. */
  measure?: ReactNode;
  /** Why: the whole claim in one sentence, capped at the reading measure. */
  evidence?: ReactNode;
  /** New or chronic: an SDK `Sparkline` (hidden under 40rem). */
  sparkline?: ReactNode;
  /** How much: the row's headline figure. */
  figure: ReactNode;
  /** How far out of line: a `Chip` in the row's tone, its tier in words in its title. */
  deviation?: ReactNode;
  /** The caps words after the deviation: "vs peers". */
  deviationNote?: ReactNode;
  /** The row whose detail is open. */
  selected?: boolean;
  /** Opens the row's detail. Without it the row is static. */
  onSelect?: () => void;
  /** What the row's button opens, said to assistive tech. Default "dialog"; false for none. */
  opens?: "dialog" | false;
  /** The row's own controls, beside its button. */
  actions?: ReactNode;
  className?: string;
}

export function FlagRow({
  rank,
  name,
  measure,
  evidence,
  sparkline,
  figure,
  deviation,
  deviationNote,
  selected = false,
  onSelect,
  opens = "dialog",
  actions,
  className
}: FlagRowProps) {
  const cells = (
    <>
      {rank != null && <span className={cx(ui.text.sm, "oxy-ui-flag-row__rank")}>{rank}</span>}
      <span className='oxy-ui-flag-row__claim'>
        <span className='oxy-ui-flag-row__identity'>
          <span className='oxy-ui-flag-row__name'>{name}</span>
          {measure != null && <span className='oxy-ui-flag-row__measure'>{measure}</span>}
        </span>
        {evidence != null && (
          <span className={cx(ui.text.sm, ui.prose, "oxy-ui-flag-row__evidence")}>{evidence}</span>
        )}
      </span>
      {sparkline != null && <span className='oxy-ui-flag-row__trend'>{sparkline}</span>}
      <span className='oxy-ui-flag-row__figures'>
        <span className='oxy-ui-flag-row__figure'>{figure}</span>
        {(deviation != null || deviationNote != null) && (
          <span className='oxy-ui-flag-row__deviation'>
            {deviation}
            {deviationNote != null && <span className={ui.eyebrow}>{deviationNote}</span>}
          </span>
        )}
      </span>
    </>
  );
  return (
    <li
      className={cx(
        "oxy-ui-flag-row",
        onSelect ? ui.rowState(selected) : selected && ui.rowSelected,
        className
      )}
    >
      {onSelect ? (
        <button
          type='button'
          onClick={onSelect}
          aria-current={selected ? "true" : undefined}
          aria-haspopup={opens || undefined}
          className={cx("oxy-ui-flag-row__main", ui.focusRingInset)}
        >
          {cells}
        </button>
      ) : (
        <div className='oxy-ui-flag-row__main'>{cells}</div>
      )}
      {actions != null && <div className='oxy-ui-flag-row__actions'>{actions}</div>}
    </li>
  );
}
