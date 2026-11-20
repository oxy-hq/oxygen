// Started from pokehouse-oxy:apps/store-ops/src/chrome/RadioSegments.tsx (INVENTORY: the only
// desk segmented component; 12 app files and 6 reference views hand-draw the same recipe). Kept:
// the radio group semantics, the keyboard, `size="touch"`, the swap to a select through
// `stripFit` when the options outgrow their room. New: `label` as an alternative to
// `labelledBy`, and `sharesRow`, as DefaultTabs has.
//
// One answer of a few, drawn as a segmented strip: the radio group a form answer or a mode
// needs, in DefaultTabs' segments. `DefaultTabs` switches what a screen SHOWS; this is for a
// field whose answer is exactly one of two to four short words, where all belong on screen and
// one press picks (a finding's flag, a chart's period). Same segments, same selected state:
// fill, weight and ink together, monochrome on the desk (decision 46 keeps the accent for the
// phone's tab bar only; the phone form of this is the kit's `PhoneSegmented`).
//
// The WAI-ARIA radio group pattern: `role="radiogroup"` over `role="radio"` buttons carrying
// `aria-checked`. One Tab stop (the checked option, the first when none is); Right/Down and
// Left/Up move to the next or previous option, wrap at the ends, and CHECK it as they move,
// which is what a radio does. A press checks the option pressed.

import type { KeyboardEvent } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import { optionText, useStripFits } from "./stripFit";

export interface SegmentedOption<T extends string> {
  value: T;
  /** Words: the select that replaces a strip too wide for its room shows them. */
  label: string;
}

export interface SegmentedProps<T extends string> {
  /** The id of the visible label that names the group. Give this or `label`. */
  labelledBy?: string;
  /** The group's name when no visible label names it. */
  label?: string;
  value: T;
  options: readonly SegmentedOption<T>[];
  onChange: (value: T) => void;
  /** `touch` on a store tablet or a phone held in one hand: the 56px crew target and crew type,
   *  full width, each option an equal share. */
  size?: "compact" | "touch";
  /** The strip shares a row with 32px controls, so it takes their height. */
  sharesRow?: boolean;
  className?: string;
}

const NEXT = new Set(["ArrowRight", "ArrowDown"]);
const PREVIOUS = new Set(["ArrowLeft", "ArrowUp"]);

export function Segmented<T extends string>({
  labelledBy,
  label,
  value,
  options,
  onChange,
  size = "compact",
  sharesRow = false,
  className
}: SegmentedProps<T>) {
  const { strip, select, fits } = useStripFits<HTMLDivElement>(
    options.map((o) => `${o.value}:${o.label}`).join("|")
  );
  const touch = size === "touch";
  const name = labelledBy ? { "aria-labelledby": labelledBy } : { "aria-label": label };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const last = options.length - 1;
    const next = NEXT.has(e.key)
      ? index === last
        ? 0
        : index + 1
      : PREVIOUS.has(e.key)
        ? index === 0
          ? last
          : index - 1
        : null;
    if (next === null) return;
    e.preventDefault();
    onChange(options[next].value);
    strip.current?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus();
  };

  // The Tab stop is the checked option, or the first when none is.
  const checkedIndex = Math.max(
    options.findIndex((o) => o.value === value),
    0
  );

  // Wider than the room it has: a select, as the strips give way.
  if (!fits) {
    return (
      <select
        ref={select}
        {...name}
        value={value}
        onChange={(e) => onChange(e.target.value as T)}
        className={cx(touch ? ui.selectTouch : ui.select, "oxy-ui-segmented__select", className)}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {optionText(o.label)}
          </option>
        ))}
      </select>
    );
  }
  return (
    <div
      ref={strip}
      role='radiogroup'
      {...name}
      className={cx(
        ui.segmentRow,
        "oxy-ui-segmented",
        touch && "oxy-ui-segmented--touch",
        className
      )}
    >
      {options.map((option, index) => {
        const on = option.value === value;
        return (
          // biome-ignore lint/a11y/useSemanticElements: a segment in a strip styled as one control; a native radio cannot take this shape.
          <button
            key={option.value}
            type='button'
            role='radio'
            aria-checked={on}
            tabIndex={index === checkedIndex ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={(e) => onKeyDown(e, index)}
            className={cx(
              ui.segment,
              "oxy-ui-segmented__option",
              sharesRow && ui.segmentSharesRow,
              on ? ui.segmentOn : ui.segmentOff
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
