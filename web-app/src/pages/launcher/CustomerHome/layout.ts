/**
 * The home's column system. Every section of the page sits in one container and
 * on the same column edges, so the ask box, the app grid and the two cards below
 * it share a left and a right edge, and the line between the two lower cards
 * falls on an edge of the app cards above them.
 *
 * The container is the same one `CriticalAlertBanner` and `NeedsAttention` use,
 * so a banner that appears lines up with everything else too.
 */
export const HOME_CONTAINER = "mx-auto w-full max-w-6xl px-6";

/**
 * The gap between columns, in the app grid, inside the one-app card and in the
 * lower row. One value, because a different gap moves a column edge.
 */
export const HOME_GAP = "gap-4";

export type AppRow = "none" | "one" | "two" | "many" | "dense";

/**
 * From this many apps the grid goes to four columns with shorter cards. At six,
 * thirds already take two full rows of tall cards (about 600 px), which pushes the
 * world model and the recent questions below the fold of a 900 px-high window; four
 * columns keep six to eight apps in two rows and the lower row in view.
 */
export const DENSE_FROM = 6;

export function appRow(appCount: number): AppRow {
  if (appCount === 0) return "none";
  if (appCount === 1) return "one";
  if (appCount === 2) return "two";
  if (appCount < DENSE_FROM) return "many";
  return "dense";
}

/**
 * Columns for the app grid. One app is a single wide card (its own art and text
 * split on the thirds, see `HomeAppCard`); two apps take half each; three to five
 * take thirds; six or more take quarters from 1280 px (thirds on a tablet, halves
 * on a phone) with compact cards.
 */
export const APP_GRID_COLUMNS: Record<Exclude<AppRow, "none">, string> = {
  one: "grid-cols-1",
  two: "grid-cols-1 sm:grid-cols-2",
  many: "grid-cols-1 sm:grid-cols-2 lg:grid-cols-3",
  dense: "grid-cols-2 md:grid-cols-3 xl:grid-cols-4"
};

/**
 * Columns for the row under the apps: the world model card and the recent
 * questions, always cut on the app grid's own edges. Under two apps the row splits
 * in half. Under quarters it splits in half too (two quarters each), and on thirds
 * below 1280 px. Otherwise it is on the thirds, the world model card taking two.
 * Without a world model card the recent questions take the whole row.
 */
export function lowerRowColumns(
  row: AppRow,
  hasWorldModel: boolean
): { row: string; worldModel: string } {
  if (!hasWorldModel) return { row: "grid-cols-1", worldModel: "" };
  if (row === "two") return { row: "grid-cols-1 lg:grid-cols-2", worldModel: "" };
  if (row === "dense") {
    return {
      row: "grid-cols-1 lg:grid-cols-3 xl:grid-cols-2",
      worldModel: "lg:col-span-2 xl:col-span-1"
    };
  }
  return { row: "grid-cols-1 lg:grid-cols-3", worldModel: "lg:col-span-2" };
}
