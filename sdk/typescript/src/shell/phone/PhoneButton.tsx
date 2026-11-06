import type { ReactNode } from "react";
import { cx } from "../cx";

export type PhoneButtonTone = "primary" | "secondary" | "danger";

export interface PhoneButtonProps {
  children: ReactNode;
  onClick?: () => void;
  tone?: PhoneButtonTone;
  disabled?: boolean;
}

/** A 48px button. Full width inside `PhoneStickyAction` and a sheet footer
 *  (two share the row), its own width elsewhere. */
export function PhoneButton({ children, onClick, tone = "primary", disabled }: PhoneButtonProps) {
  return (
    <button
      type='button'
      // motion.css times its press feedback from this slot.
      data-slot='button'
      disabled={disabled}
      onClick={onClick}
      className={cx("oxy-shell-scope oxy-phone-btn", `oxy-phone-btn--${tone}`)}
    >
      {children}
    </button>
  );
}
