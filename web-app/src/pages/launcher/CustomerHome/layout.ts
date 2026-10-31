/**
 * The home's frame. Apps and widgets each have a place of their own, so neither
 * pushes the other off the screen:
 *
 * - From `lg` (1024 px), two columns that scroll on their own: the greeting, the
 *   apps and what is below them in the left two thirds; the Today rail, every
 *   widget, in the right third.
 * - Under `lg`, one column: the greeting, the apps, then Today stacked under
 *   them (two cards to a row on a tablet), what needs someone first.
 *
 * The container is the same one `CriticalAlertBanner` uses, so a banner that
 * appears lines up with everything else.
 */
export const HOME_CONTAINER = "mx-auto w-full max-w-6xl px-6";

/** The gap between grid columns on the home. One value, so column edges line up. */
export const HOME_GAP = "gap-4";

/** From here the Today rail sits beside the apps; under it, Today stacks under them. */
export const HOME_RAIL_QUERY = "(min-width: 1024px)";
