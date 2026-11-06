// A list row: "needs you" items, settings, a feed. When it is clickable the
// row's text is a button whose hit area is stretched over the whole row (CSS
// `::after`), so a tap anywhere opens it while a control in `trailing` stays
// its own target — no button nested in a button.

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ChevronRightIcon } from "./icons";
import { type PhoneTone, present, toneClass } from "./tone";

export interface PhoneRowProps {
  title: ReactNode;
  subtitle?: ReactNode;
  /** A status dot before the text. */
  tone?: PhoneTone;
  /** Right side: a value, a pill, a `PhoneHint`, a small button. */
  trailing?: ReactNode;
  /** Makes the row a button, with a chevron. */
  onClick?: () => void;
}

export function PhoneRow({ title, subtitle, tone, trailing, onClick }: PhoneRowProps) {
  const text = (
    <>
      {tone && <span className={cx("oxy-phone-row__dot", toneClass(tone))} aria-hidden='true' />}
      <span className='oxy-phone-row__text'>
        <span className='oxy-phone-row__title'>{title}</span>
        {present(subtitle) && <span className='oxy-phone-row__subtitle'>{subtitle}</span>}
      </span>
    </>
  );
  return (
    <div className={cx("oxy-shell-scope oxy-phone-row", onClick && "oxy-phone-row--button")}>
      {onClick ? (
        <button type='button' className='oxy-phone-row__main' onClick={onClick}>
          {text}
        </button>
      ) : (
        <div className='oxy-phone-row__main'>{text}</div>
      )}
      {present(trailing) && <div className='oxy-phone-row__trailing'>{trailing}</div>}
      {onClick && (
        <span className='oxy-phone-row__chevron' aria-hidden='true'>
          <ChevronRightIcon />
        </span>
      )}
    </div>
  );
}
