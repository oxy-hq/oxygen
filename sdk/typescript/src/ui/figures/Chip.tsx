// Started from oxy-hq/reference-app@3caad1f `src/components/Chip.tsx` (INVENTORY section 2: the
// reference is the fullest of seven copies, 125 lines against 14-67). Kept: `tone` from
// `chipTone`, `color` for an accent that is not a status, `icon`, `onRemove` + `removeLabel`
// with a × whose hover tints the chip's OWN ink, `title`, `className`. Added: `disabled`, from
// warehouse's copy (`apps/warehouse/src/components/Chip.tsx`, a form that locks while it
// submits), and the ×'s 44px reach on a touch screen (decision 16), drawn as a reach around the
// 16px square so the chip keeps its size. `Pill` in other apps is this piece (BEYOND-GALLERY 17).
//
// A chip is the small filled tag beside the thing it qualifies: a status, a name, a zone.
//
// **`tone` is what the chip MEANS**, one vocabulary resolved through `chipTone`, so "warning"
// is the same tint on every screen. **`color` is a raw colour** for a chip that wears a
// palette's own accent rather than a verdict (a temperature zone): the chip fills with it at
// 15% and sets its text to it. Pass one or the other. A chip with a tone always carries its
// word: the tint is never the only channel.
//
// **The remove ×** is the one place a chip is something you take away (a settings list's
// entries). Its hover is `currentColor` at 15% behind the ×, so a neutral, a warning and an
// accent chip each darken in their own hue; the × steps from 70% to full ink. Name the ENTRY
// in `removeLabel` ("Remove Cascade Freight from shipping companies"), or a list of twelve
// reads "Remove, Remove, Remove".

import type { ComponentPropsWithRef, CSSProperties, ReactNode } from "react";
import { cx } from "../cx";
import { XIcon } from "../icons";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";

/** How much of a raw `color` goes into the fill: the 15% both transcribed callers wrote out. */
const FILL = 15;

export interface ChipProps extends Omit<ComponentPropsWithRef<"span">, "children" | "color"> {
  /** What the chip means. One definition, every screen: see `chipTone`. */
  tone?: StatusTone;
  /** A raw colour for a chip whose colour is not a status. Pass this OR `tone`. */
  color?: string;
  /** In front of the label, at `ui.iconInline`. Decorative: the label says what it means. */
  icon?: ReactNode;
  /** Makes the chip removable: a trailing × that calls this. */
  onRemove?: () => void;
  /** The ×'s accessible name. Required with `onRemove`; name the entry. */
  removeLabel?: string;
  /** Locks the × (a form that is submitting). */
  disabled?: boolean;
  children: ReactNode;
}

export function Chip({
  tone,
  color,
  icon,
  onRemove,
  removeLabel,
  disabled,
  className,
  style,
  children,
  ...rest
}: ChipProps) {
  const accent: CSSProperties | undefined = color
    ? { background: `color-mix(in srgb, ${color} ${FILL}%, transparent)`, color }
    : undefined;
  return (
    <span
      {...rest}
      className={cx(
        ui.tag,
        "oxy-ui-chip",
        tone && ui.chipTone[tone],
        (icon != null || onRemove !== undefined) && "oxy-ui-chip--gap",
        onRemove && "oxy-ui-chip--removable",
        className
      )}
      style={accent ? { ...style, ...accent } : style}
      data-tone={tone}
    >
      {icon}
      {children}
      {onRemove && (
        <button
          type='button'
          className={cx("oxy-ui-chip__remove", ui.focusRing)}
          onClick={onRemove}
          disabled={disabled}
          aria-label={removeLabel}
        >
          <XIcon className={ui.iconInline} />
        </button>
      )}
    </span>
  );
}
