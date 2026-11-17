// The shell's 44px tap areas on a touch screen (decision 16: every desk app gives each control
// 44px on a touch screen; enrolled store tablets excepted). shell.css draws them under
// `(pointer: coarse)` with theme.css applied, unless `<html data-oxy-touch-targets="off">`. This
// is the screen-level way to set that attribute, for a screen that is not a person's desk, such as
// a store's enrolled tablet.

import { useLayoutEffect } from "react";

/** On `<html>`: `"off"` draws the shell's chrome at its drawn size on a touch screen too. */
export const OXY_TOUCH_TARGETS_ATTRIBUTE = "data-oxy-touch-targets";

/**
 * `false` turns the shell's 44px tap areas off while this component is mounted (`true` or
 * `undefined` leaves them as they are). When it unmounts, whatever was set before (an
 * `index.html` attribute, or nothing) comes back. Use it in a component that wraps the shell, such
 * as a kiosk frame around the app; inside the shell, `<OxyShell touchTargets={false}>` does the
 * same.
 */
export function useOxyTouchTargets(enabled: boolean | undefined): void {
  useLayoutEffect(() => {
    if (enabled !== false) return;
    const root = document.documentElement;
    const before = root.getAttribute(OXY_TOUCH_TARGETS_ATTRIBUTE);
    root.setAttribute(OXY_TOUCH_TARGETS_ATTRIBUTE, "off");
    return () => {
      if (before === null) root.removeAttribute(OXY_TOUCH_TARGETS_ATTRIBUTE);
      else root.setAttribute(OXY_TOUCH_TARGETS_ATTRIBUTE, before);
    };
  }, [enabled]);
}
