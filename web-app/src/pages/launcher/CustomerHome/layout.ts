/**
 * The home's frames.
 *
 * Home v2 (`home_v3` off): apps and widgets each have a place of their own, so
 * neither pushes the other off the screen, and the page is the one scroller:
 *
 * - From `lg` (1024 px), two columns: the greeting, the ask box and the apps in
 *   the main one, which the app grid fills with as many columns as fit; a side
 *   column (`w-80`, `w-96` from `xl`) holding Today, then recent questions and the data map, so
 *   the side is never a tall empty strip and the apps never share their column.
 * - Under `lg`, one column: the greeting, the apps, then Today stacked under
 *   them (two lines to a row on a tablet), what needs someone first.
 *
 * The container is the same one `CriticalAlertBanner` uses, so a banner that
 * appears lines up with everything else; the world model page lines up with it too.
 */
export const HOME_CONTAINER = "mx-auto w-full max-w-7xl px-6";

/** The gap between grid columns on the v2 home. One value, so column edges line up. */
export const HOME_GAP = "gap-4";

/** From here the v2 side column sits beside the apps; under it, everything stacks under them. */
export const HOME_RAIL_QUERY = "(min-width: 1024px)";

/**
 * Home v3's one column: narrower than the container, so the ask box, the apps
 * and Today read top to bottom as one list rather than spread over a wide screen.
 */
export const HOME_COLUMN = "mx-auto flex w-full max-w-5xl flex-col gap-10 px-6 pt-8 pb-12";
