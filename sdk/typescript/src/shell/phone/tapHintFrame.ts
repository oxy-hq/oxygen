// The tap form of a hint: where a layout has one, a hint that a pointer reads by hovering is drawn
// as an ⓘ whose tap opens the words. `@oxy-hq/sdk/ui` exports it (containers' HoverHint, for
// `Stat`'s tip); it lives here, a context and nothing else, so `OxyShell` can provide it on a phone
// (`PhoneTapHintFrame`, 2.31.1) without the shell carrying the desk UI. Both entries take this one
// module from the shared chunk, so an app's `TapHintFrame` from the ui entry is the shell's.

import { createContext, type ReactNode, useContext } from "react";

/**
 * The tap form of a hint, where the layout has one: an ⓘ whose tap opens the words (the kit's
 * `PhoneHint`). `label` is the ⓘ's name ("About Income"), `text` the hint. `OxyShell` provides it
 * on a phone layout; an app's own provider, inside the shell or around it, wins. Nothing provides
 * it on a desk.
 */
export const TapHintFrame = createContext<((label: string, text: string) => ReactNode) | null>(
  null
);

/** The layout's tap form for a hint, or null on the desk. See `TapHintFrame`. */
export function useTapHint() {
  return useContext(TapHintFrame);
}
