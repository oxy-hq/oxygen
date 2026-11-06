import type { ReactNode } from "react";
import { present } from "./tone";

export interface PhoneSectionProps {
  /** The 11px label; may carry a `PhoneHint` after the words. */
  title: ReactNode;
  /** Right of the label, e.g. a "See all ›" button. */
  action?: ReactNode;
  children: ReactNode;
}

/** A labelled group. `PhoneRow`s directly inside join into one list. */
export function PhoneSection({ title, action, children }: PhoneSectionProps) {
  return (
    <section className='oxy-shell-scope oxy-phone-section'>
      <div className='oxy-phone-section__header'>
        <h2 className='oxy-phone-section__title'>{title}</h2>
        {present(action) && <div className='oxy-phone-section__action'>{action}</div>}
      </div>
      <div className='oxy-phone-section__body'>{children}</div>
    </section>
  );
}
