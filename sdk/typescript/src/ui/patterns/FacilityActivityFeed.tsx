// Started from oxy-hq/reference-app@81ee8aa `src/components/FacilityActivityFeed.tsx` (INVENTORY
// section 4: 0 copies, no origin named; transcribed there from the warehouse module design's live
// activity strip). Kept: a flat list of one-line rows (time, who, what, and where, a location code,
// on the right) in a box that scrolls, a rule between rows and none under the last; tabular
// figures; the secondary ink, with who and where in full ink; a row that names a place is a
// button, and pressing it hands the place to the screen (`onSelectLocation`), which selects it
// elsewhere; the focus ring outside the row, inside the box's own 2px of room. Kept apart from a
// movement history's connected timeline on purpose (the reference's note): different shape,
// different job. New: the entry shape is the reference's own (`ts`, `actor`, `text`, `loc`), so
// a screen's entries pass in as they are; a row with no place is plain text, not a disabled
// button (nothing to press is not a control that is off); the rows are a named list; hover is
// CSS, not state; `empty` says so when there is nothing yet; the box's height is a prop.

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface ActivityFeedEntry {
  /** When: "09:41". */
  ts: ReactNode;
  /** Who. */
  actor: ReactNode;
  /** What happened. Cut to one line. */
  text: ReactNode;
  /** Where: a location code. A row with one is a button that selects it. */
  loc?: string | null;
  /** A stable key, when two entries could otherwise be told apart only by position. */
  id?: string;
}

export interface FacilityActivityFeedProps {
  entries: readonly ActivityFeedEntry[];
  /** Selects the location a row names, elsewhere on the screen. */
  onSelectLocation?: (loc: string) => void;
  /** The list's name: "Live activity". */
  label?: string;
  /** Said for an empty feed. Default "No activity yet." */
  empty?: ReactNode;
  /** The box's height before it scrolls; any CSS length. Default: the height its parent gives. */
  maxHeight?: string;
  className?: string;
}

export function FacilityActivityFeed({
  entries,
  onSelectLocation,
  label,
  empty = "No activity yet.",
  maxHeight,
  className
}: FacilityActivityFeedProps) {
  if (entries.length === 0)
    return <p className={cx(ui.note, "oxy-ui-activity-feed__empty", className)}>{empty}</p>;
  return (
    <ul
      aria-label={label}
      className={cx(ui.text.sm, "oxy-ui-activity-feed", className)}
      style={maxHeight ? { maxHeight } : undefined}
    >
      {entries.map((a, i) => {
        const cells = (
          <>
            <span>{a.ts}</span>
            <span className='oxy-ui-activity-feed__ink'>{a.actor}</span>
            <span className='oxy-ui-activity-feed__text'>{a.text}</span>
            {a.loc && <span className='oxy-ui-activity-feed__ink'>{a.loc}</span>}
          </>
        );
        const loc = a.loc;
        return (
          <li key={a.id ?? i} className='oxy-ui-activity-feed__item'>
            {loc && onSelectLocation ? (
              <button
                type='button'
                onClick={() => onSelectLocation(loc)}
                className={cx(
                  "oxy-ui-activity-feed__row oxy-ui-activity-feed__row--action",
                  ui.focusRing
                )}
              >
                {cells}
              </button>
            ) : (
              <span className='oxy-ui-activity-feed__row'>{cells}</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
