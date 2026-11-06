// A desk-table row as a card: its name, its status, and the two or three
// numbers that matter, readable without scrolling sideways. Tap opens the
// detail (usually a PhoneSheet); `trailing` holds the row's own action (a
// 44px Post button), which stays its own target above the card's tap area.

import type { ReactNode } from "react";
import { cx } from "../cx";
import { ChevronRightIcon } from "./icons";
import { type PhoneTone, present, toneClass } from "./tone";

export interface PhoneCardStatus {
  label: string;
  tone: PhoneTone;
}

export interface PhoneCardFigure {
  label: string;
  value: ReactNode;
}

export interface PhoneCardProps {
  title: ReactNode;
  subtitle?: ReactNode;
  /** A pill on the title line ("Posted", "Late"). */
  status?: PhoneCardStatus;
  /** Up to three label-over-value figures; any more are dropped. */
  figures?: PhoneCardFigure[];
  trailing?: ReactNode;
  onClick?: () => void;
}

const MAX_FIGURES = 3;

export function PhoneCard({ title, subtitle, status, figures, trailing, onClick }: PhoneCardProps) {
  const text = (
    <>
      <span className='oxy-phone-card__titleline'>
        <span className='oxy-phone-card__title'>{title}</span>
        {status && (
          <span className={cx("oxy-phone-pill", toneClass(status.tone))}>{status.label}</span>
        )}
      </span>
      {present(subtitle) && <span className='oxy-phone-card__subtitle'>{subtitle}</span>}
    </>
  );
  const shown = figures?.slice(0, MAX_FIGURES) ?? [];
  return (
    <div className={cx("oxy-shell-scope oxy-phone-card", onClick && "oxy-phone-card--button")}>
      <div className='oxy-phone-card__head'>
        {onClick ? (
          <button type='button' className='oxy-phone-card__main' onClick={onClick}>
            {text}
          </button>
        ) : (
          <div className='oxy-phone-card__main'>{text}</div>
        )}
        {present(trailing) && <div className='oxy-phone-card__trailing'>{trailing}</div>}
        {onClick && (
          <span className='oxy-phone-card__chevron' aria-hidden='true'>
            <ChevronRightIcon />
          </span>
        )}
      </div>
      {shown.length > 0 && (
        <dl className='oxy-phone-card__figures'>
          {shown.map((figure) => (
            <div key={figure.label} className='oxy-phone-card__figure'>
              <dt>{figure.label}</dt>
              <dd>{figure.value}</dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  );
}
