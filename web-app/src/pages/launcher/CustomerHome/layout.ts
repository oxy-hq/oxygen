/**
 * The home's frame. Apps and widgets each have a place of their own, so neither
 * pushes the other off the screen, and the page is the one scroller:
 *
 * - From `lg` (1024 px), two columns: the greeting, the ask box and the apps in
 *   the main one, which the app grid fills with as many columns as fit; a side
 *   column (`w-80`, `w-96` from `xl`) holding Today, then recent questions and the data map, so
 *   the side is never a tall empty strip and the apps never share their column.
 * - Under `lg`, one column: the greeting, the apps, then Today stacked under
 *   them (two lines to a row on a tablet), what needs someone first.
 *
 * The container is the same one `CriticalAlertBanner` uses, so a banner that
 * appears lines up with everything else.
 */
export const HOME_CONTAINER = "mx-auto w-full max-w-7xl px-6";

/** The gap between grid columns on the home. One value, so column edges line up. */
export const HOME_GAP = "gap-4";

/** From here the side column sits beside the apps; under it, everything stacks under them. */
export const HOME_RAIL_QUERY = "(min-width: 1024px)";
