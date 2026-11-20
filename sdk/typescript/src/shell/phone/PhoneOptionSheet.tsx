// Started from oxy-hq/reference-app@3caad1f `src/phone/OptionSheet.tsx` (BEYOND-GALLERY row 14: it
// states the 44px+ rows; six copies in three repos, all on the kit's PhoneSheet). Kept: its props
// (`open`, `onClose`, `title`, `backLabel`, `options` with an optional `detail`, `value`,
// `onChange`, `children` before the options), the 48px rows on a card with a hairline between
// them, the chosen row's weight and check in the row's own ink (blue is links, focus and the
// primary action), the caller closing the sheet. Changed: the semantics, which BEYOND-GALLERY
// left open. Every copy marked the chosen row `aria-pressed`, which announces a toggle button
// that a second press would release; Watchlist's added a hidden "(selected)". A choice of one is
// a radio group: `role="radiogroup"` named by the sheet's title, `role="radio"` rows with
// `aria-checked`, read as "Name, radio button, checked, 2 of 5", as the kit's PhoneSegmented
// reads.
//
// One Tab stop (the chosen row, the first when none is). The arrows, Home and End move focus
// between the rows WITHOUT choosing: here a choice commits the answer and the callers close the
// sheet on it, so a choice that followed focus would close the sheet on the first arrow (APG
// leaves selection-follows-focus optional where it does work). A tap, Enter or Space chooses.
// Back, Escape and the phone's Back dismiss without choosing (PhoneSheet).
//
// The phone form of a select or of a table's sortable headings:
//
//   <PhoneOptionSheet open={open} onClose={close} title="Sort stores" backLabel="Stores"
//     options={SORTS} value={sort} onChange={(v) => { setSort(v); close(); }} />

import { type KeyboardEvent, type ReactNode, useRef } from "react";
import { cx } from "../cx";
import { PhoneSheet } from "./PhoneSheet";
import { present } from "./tone";

export interface PhoneOption<T extends string = string> {
  value: T;
  label: string;
  /** A second line under the label, in the muted ink. */
  detail?: ReactNode;
}

export interface PhoneOptionSheetProps<T extends string = string> {
  open: boolean;
  /** Back, Escape and the phone's Back: dismiss without choosing. */
  onClose: () => void;
  title: string;
  backLabel?: string;
  options: readonly PhoneOption<T>[];
  /** The chosen value; null when none is. */
  value: T | null;
  /** A row chosen. The sheet does not close itself: close it here when the choice is final. */
  onChange: (value: T) => void;
  /** Anything the sheet says before its options. */
  children?: ReactNode;
}

/** Phosphor Check, regular weight (as the kit's other glyphs, icons.tsx). */
function CheckGlyph() {
  return (
    <svg width={20} height={20} viewBox='0 0 256 256' fill='currentColor' aria-hidden='true'>
      <path d='M229.66,77.66l-128,128a8,8,0,0,1-11.32,0l-56-56a8,8,0,0,1,11.32-11.32L96,188.69,218.34,66.34a8,8,0,0,1,11.32,11.32Z' />
    </svg>
  );
}

const MOVES: Record<string, (at: number, last: number) => number> = {
  ArrowDown: (at, last) => (at === last ? 0 : at + 1),
  ArrowRight: (at, last) => (at === last ? 0 : at + 1),
  ArrowUp: (at, last) => (at === 0 ? last : at - 1),
  ArrowLeft: (at, last) => (at === 0 ? last : at - 1),
  Home: () => 0,
  End: (_at, last) => last
};

export function PhoneOptionSheet<T extends string = string>({
  open,
  onClose,
  title,
  backLabel,
  options,
  value,
  onChange,
  children
}: PhoneOptionSheetProps<T>) {
  const groupRef = useRef<HTMLDivElement>(null);
  const stop = Math.max(
    options.findIndex((o) => o.value === value),
    0
  );

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const move = MOVES[e.key];
    if (!move) return;
    e.preventDefault();
    const rows = groupRef.current?.querySelectorAll<HTMLElement>('[role="radio"]');
    rows?.[move(index, options.length - 1)]?.focus();
  };

  return (
    <PhoneSheet open={open} onClose={onClose} title={title} backLabel={backLabel}>
      {present(children) && children}
      <div ref={groupRef} role='radiogroup' aria-label={title} className='oxy-phone-options'>
        {options.map((option, index) => {
          const on = option.value === value;
          return (
            // biome-ignore lint/a11y/useSemanticElements: a 48px row with a label, a detail and a check; a native radio cannot take this shape.
            <button
              key={option.value}
              type='button'
              role='radio'
              aria-checked={on}
              tabIndex={index === stop ? 0 : -1}
              className={cx("oxy-phone-options__option", on && "oxy-phone-options__option--on")}
              onClick={() => onChange(option.value)}
              onKeyDown={(e) => onKeyDown(e, index)}
            >
              <span className='oxy-phone-options__text'>
                <span className='oxy-phone-options__label'>{option.label}</span>
                {present(option.detail) && (
                  <span className='oxy-phone-options__detail'>{option.detail}</span>
                )}
              </span>
              {on && (
                <span className='oxy-phone-options__check'>
                  <CheckGlyph />
                </span>
              )}
            </button>
          );
        })}
      </div>
    </PhoneSheet>
  );
}
