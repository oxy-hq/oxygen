// Started from oxy-hq/reference-app@3caad1f `src/components/StatusDot.tsx` (INVENTORY section 2:
// the reference is the fullest of five copies). Kept: the 7px round dot on the `--dot-*` fills,
// no glow, `status` + `label`. Changed by decision 38b ("status dots, round, always with a
// word"): the word is printed beside the dot, not hidden for screen readers only, as Quadrogen's
// `StatusWord` already drew it (`apps/command-center/src/components/StatusWord.tsx`). Taken from
// the app copies: `neutral` (Store Ops, Command Center, Locations: a state that is a fact, not a
// verdict, on the theme's neutral series), `quiet` (Watchlist, decision 7: the quieter of two
// steps in one status, a 1px ring in the same colour), `wordId` and `dotClassName` (Store Ops:
// a control that takes the state as its description; a dot that pulses).
//
// **Round on purpose.** Every container is square; a dot is the exception because it has
// nothing inside it, and squared off at 7px it reads as a cut glyph (decision 38b keeps it).
//
// **Never colour alone.** The word is the state: "Within range", "Live", "Opened by a failed
// check". It takes the surrounding text's size and ink, so a dot in a table cell reads in the
// cell's voice; the dot carries the hue.

import type { ComponentPropsWithRef } from "react";
import { cx } from "../cx";
import type { StatusTone } from "../tone";

/** The reference's name for a dot's status: the five tones. */
export type DotStatus = StatusTone;

export interface StatusDotProps extends Omit<ComponentPropsWithRef<"span">, "children"> {
  status: DotStatus;
  /** The state in words, printed beside the dot: "Within range", "Live data". */
  label: string;
  /** The quieter step inside one status: the dot drawn as a ring in the same colour. */
  quiet?: boolean;
  /** An id for the word, for a control that takes the state as its `aria-describedby`. */
  wordId?: string;
  /** For a dot that moves (a failed check's pulse). */
  dotClassName?: string;
}

export function StatusDot({
  status,
  label,
  quiet = false,
  wordId,
  dotClassName,
  className,
  ...rest
}: StatusDotProps) {
  return (
    <span {...rest} className={cx("oxy-ui-status-dot", className)} data-status={status}>
      <span
        className={cx(
          "oxy-ui-status-dot__dot",
          `oxy-ui-status-dot__dot--${status}`,
          quiet && "oxy-ui-status-dot__dot--quiet",
          dotClassName
        )}
        aria-hidden='true'
      />
      <span id={wordId} className='oxy-ui-status-dot__word'>
        {label}
      </span>
    </span>
  );
}
