import type { ReactNode } from "react";
import { present } from "./tone";

export interface PhoneCardListProps {
  /** `PhoneCard`s. */
  children: ReactNode;
  /** One line on a card above the list: "14 stores · $724,102 · 14/14 posted". */
  summary?: ReactNode;
}

export function PhoneCardList({ children, summary }: PhoneCardListProps) {
  return (
    <div className='oxy-shell-scope oxy-phone-cards'>
      {present(summary) && <div className='oxy-phone-cards__summary'>{summary}</div>}
      {children}
    </div>
  );
}
