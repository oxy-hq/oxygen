/**
 * The token table's columns: how wide each is, and which give way as the table's own box narrows.
 *
 * The box is measured, not the window: Settings shows the list in a pane 736px wide however large
 * the screen is, so a viewport breakpoint would never fire. At that width the Token column is
 * left out, since the name cell's title and the Activity drawer both carry the token's prefix.
 * Last used and then Kind follow on a pane narrower still. The name, what the token reaches,
 * when it dies and every action are always on the row.
 *
 * `width` goes on the header cell of a fixed-layout table and `shown` on the header and body cell
 * alike. Access has no width: it takes what the others leave, and truncates.
 */
export const TOKEN_COLUMNS = {
  name: { label: "Name", width: "w-28", shown: "" },
  kind: { label: "Kind", width: "w-25", shown: "hidden @xl:table-cell" },
  token: { label: "Token", width: "w-36", shown: "hidden @5xl:table-cell" },
  access: { label: "Access", width: "", shown: "" },
  expiry: { label: "Expiry", width: "w-26", shown: "" },
  lastUsed: { label: "Last used", width: "w-22", shown: "hidden @2xl:table-cell" }
} as const;

/** Extend, Activity, Revoke and the menu, each in its own place. */
export const ACTIONS_COLUMN_WIDTH = "w-48";
