// Started from oxy-hq/reference-app@81ee8aa `src/components/ReviewList.tsx` (INVENTORY section 4: one
// copy, Watchlist's `apps/watchlist/src/components/ReviewList.tsx`, which the reference grew from,
// diff 112). Kept: an eyebrow title and the caveat ABOVE the list, where it is read first; one card
// per account (`card` + `panel`), never a stack of hairlines, laid across the width by
// `repeat(auto-fit, minmax(18rem, 1fr))` so a single item is a full-width card and the columns
// follow the width; two fixed lines on each card (the rating line, then the meta line) so every
// card breaks in the same place; the rating's figure beside its picture; the flag ("No reply") on
// the rating line, right, in the warning ink; the text verbatim, never cut, its line breaks kept;
// a loading line while it reads; nothing at all for an empty list. New, as a layout with slots:
// each item's parts are the caller's nodes (the rating picture, its figure, the flag, the meta,
// the text), as are the title, the caveat and every word; `empty` says something for an empty
// list where the screen wants it said, and the loading line is announced (`role="status"`).
//
// The stars stay the caller's: the reference draws them at Phosphor's fill weight, which the SDK
// keeps for AppMark alone; a rating picture is `role="img"` with its words ("2 out of 5 stars").

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import { type StatusTone, toneText } from "../tone";

export interface ReviewListItem {
  key: string;
  /** The rating as a picture (`role="img"`, named in words). */
  rating?: ReactNode;
  /** The rating as a figure, beside the picture: "2 stars". */
  score?: ReactNode;
  /** On the rating line, right, in `flagTone`: "No reply". */
  flag?: ReactNode;
  /** The second line: where and when ("Delivery app · 2026-11-02"). */
  meta?: ReactNode;
  /** What they wrote, verbatim: line breaks kept, never cut. */
  message: ReactNode;
}

export interface ReviewListProps {
  /** The eyebrow over the list: "What unhappy guests said". */
  title: ReactNode;
  /** The caveat, read before the list: what the list is and is not a sample of. */
  note?: ReactNode;
  items: readonly ReviewListItem[];
  /** Still reading: the loading line shows in place of the list. */
  loading?: boolean;
  /** The loading line. Default "Loading…". */
  loadingText?: ReactNode;
  /** Said for an empty list. Default nothing: an empty list renders nothing. */
  empty?: ReactNode;
  /** The flag's tone. Default warning. */
  flagTone?: StatusTone;
  /** The narrowest a card may get before the grid drops a column. Default 18rem. */
  min?: string;
  className?: string;
}

export function ReviewList({
  title,
  note,
  items,
  loading = false,
  loadingText = "Loading…",
  empty,
  flagTone = "warning",
  min = "18rem",
  className
}: ReviewListProps) {
  if (loading)
    return (
      <p role='status' className={cx(ui.note, className)}>
        {loadingText}
      </p>
    );
  if (items.length === 0 && empty == null) return null;
  return (
    <div className={cx("oxy-ui-review-list", className)}>
      <h3 className={cx(ui.eyebrow, "oxy-ui-review-list__title")}>{title}</h3>
      {note != null && <p className={cx(ui.note, "oxy-ui-review-list__note")}>{note}</p>}
      {items.length === 0 ? (
        <p className={cx(ui.note, "oxy-ui-review-list__empty")}>{empty}</p>
      ) : (
        <ul
          className='oxy-ui-review-list__grid'
          style={{ gridTemplateColumns: `repeat(auto-fit, minmax(${min}, 1fr))` }}
        >
          {items.map((r) => (
            <li key={r.key} className={cx(ui.card, ui.panel, "oxy-ui-review-list__item")}>
              <div className='oxy-ui-review-list__lines'>
                <div className='oxy-ui-review-list__rating'>
                  {r.rating}
                  {r.score != null && (
                    <span className={cx(ui.text.sm, "oxy-ui-review-list__score")}>{r.score}</span>
                  )}
                  {r.flag != null && (
                    <span
                      className={cx(ui.text.sm, toneText[flagTone], "oxy-ui-review-list__flag")}
                    >
                      {r.flag}
                    </span>
                  )}
                </div>
                {r.meta != null && (
                  <p className={cx(ui.note, "oxy-ui-review-list__meta")}>{r.meta}</p>
                )}
              </div>
              <p className={cx(ui.text.sm, "oxy-ui-review-list__message")}>{r.message}</p>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
