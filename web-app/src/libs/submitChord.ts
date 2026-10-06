/**
 * The chord that confirms a form or an approval from the keyboard: Command+Enter on an Apple
 * platform, Control+Enter anywhere else. A plain Enter is never it, so a key meant for another
 * window (a terminal, a search box) can't confirm anything.
 */

type NavigatorWithUAData = Navigator & { userAgentData?: { platform?: string } };

/** Whether the keyboard has a Command key. Read at call time, so a test can change platform. */
export const isApplePlatform = (): boolean => {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as NavigatorWithUAData;
  return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform || nav.platform || "");
};

/** The chord as `aria-keyshortcuts` spells it, for the button it presses. */
export const submitChordShortcut = (apple = isApplePlatform()): string =>
  apple ? "Meta+Enter" : "Control+Enter";

type ChordEvent = Pick<
  KeyboardEvent,
  "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "repeat" | "isComposing"
>;

/** A held key repeats, and a held chord must not confirm twice. */
export const isSubmitChord = (event: ChordEvent, apple = isApplePlatform()): boolean => {
  if (event.key !== "Enter" || event.repeat || event.isComposing) return false;
  if (event.altKey || event.shiftKey) return false;
  return apple ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
};
