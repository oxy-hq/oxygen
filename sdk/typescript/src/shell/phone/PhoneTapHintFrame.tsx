// `OxyShell`'s tap form for hints (2.31.1). A `Stat` with a `tip` drew its phone form (the kit's
// tap ⓘ on the label line) only where the app provided `TapHintFrame`; only Poke House Bookkeeping
// did, so every other app's phone showed the desk ⓘ with a hover bubble, which a finger cannot
// open. The shell now provides it wherever the app takes its phone layout.
//
// Why the shell, and not a coarse-pointer fallback in `Stat`: the frame follows `usePhoneLayout()`,
// the very signal an app switches to its phone view on, so the desk is unchanged everywhere,
// including a touch tablet or a kiosk on its desk layout, which a coarse-pointer test would have
// flipped; the tap form is the kit's `PhoneHint`, the one Bookkeeping already ships, not a second
// one drawn in ui.css; and an app that provides its own frame keeps it, because the nearest
// provider wins inside the shell and one around the shell is passed on.

import { type ReactNode, useContext } from "react";
import { PhoneHint } from "./PhoneHint";
import { TapHintFrame } from "./tapHintFrame";
import { usePhoneLayout } from "./usePhoneLayout";

/** The kit's tap form for a hint: `PhoneHint`, the ⓘ a tap opens. */
export function phoneTapHint(label: string, text: string): ReactNode {
  return <PhoneHint label={label}>{text}</PhoneHint>;
}

/**
 * On a phone layout, the kit's tap form for every hint under it that has none from further up; on
 * a desk, whatever is further up (nothing, usually), so the desk draws as before. Always the same
 * element, so crossing the breakpoint re-renders the hints and never remounts the app. A server
 * render is a desk (`usePhoneLayout`'s server answer).
 */
export function PhoneTapHintFrame({ children }: { children: ReactNode }) {
  const outer = useContext(TapHintFrame);
  const phone = usePhoneLayout();
  return (
    <TapHintFrame.Provider value={phone ? (outer ?? phoneTapHint) : outer}>
      {children}
    </TapHintFrame.Provider>
  );
}
