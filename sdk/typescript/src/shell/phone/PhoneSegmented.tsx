import { type KeyboardEvent, useRef } from "react";
import { cx } from "../cx";

export interface PhoneSegmentedOption<T extends string = string> {
  value: T;
  label: string;
}

export interface PhoneSegmentedProps<T extends string = string> {
  /** Two to four; more belong in a select or a sheet. */
  options: PhoneSegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** The group's name, read by screen readers ("View"). */
  ariaLabel?: string;
}

/** Equal 44px segments for a small switch: List | Map, Day | Week | Month.
 *  A radio group: one tab stop, arrow keys move the choice. */
export function PhoneSegmented<T extends string = string>({
  options,
  value,
  onChange,
  ariaLabel
}: PhoneSegmentedProps<T>) {
  const groupRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!step || options.length === 0) return;
    e.preventDefault();
    const at = options.findIndex((o) => o.value === value);
    // Nothing chosen yet: the first arrow lands on an end, not one past it.
    const next =
      at < 0 ? (step > 0 ? 0 : options.length - 1) : (at + step + options.length) % options.length;
    onChange(options[next].value);
    groupRef.current?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
  };

  // One tab stop always, even while `value` matches no option (still loading).
  const tabStop = options.some((o) => o.value === value) ? value : options[0]?.value;

  return (
    <div
      ref={groupRef}
      role='radiogroup'
      aria-label={ariaLabel}
      className='oxy-shell-scope oxy-phone-segmented'
      onKeyDown={onKeyDown}
    >
      {options.map((option) => {
        const checked = option.value === value;
        return (
          // biome-ignore lint/a11y/useSemanticElements: a segmented control is a button row styled as one control; a native radio cannot take this shape.
          <button
            key={option.value}
            type='button'
            role='radio'
            aria-checked={checked}
            tabIndex={option.value === tabStop ? 0 : -1}
            className={cx(
              "oxy-phone-segmented__option",
              checked && "oxy-phone-segmented__option--active"
            )}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
