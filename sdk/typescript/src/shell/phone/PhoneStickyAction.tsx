import type { ReactNode } from "react";

export interface PhoneStickyActionProps {
  /** One or two `PhoneButton`s. */
  children: ReactNode;
}

/** The action a screen exists for (Post, Save, Order), pinned where a thumb
 *  reaches it. Pass it as `PhonePage`'s `stickyAction` so it sits above the tab
 *  bar and the page clears it; on its own it is fixed to the bottom edge. */
export function PhoneStickyAction({ children }: PhoneStickyActionProps) {
  return <div className='oxy-shell-scope oxy-phone-sticky'>{children}</div>;
}
