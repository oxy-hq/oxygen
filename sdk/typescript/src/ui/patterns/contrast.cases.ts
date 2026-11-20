// The patterns group's contrast cases: every on, selected, hover and disabled state its pieces draw,
// light and dark (contrast.test.ts runs them). See CONVENTIONS.md for the shape.
//
// Already measured elsewhere and not repeated: the recipes (src/ui/contrast.cases.ts) for the
// eyebrow and the note on a card and the page and the eyebrow in a selected row (section heads,
// meta lines, footnotes, the deviation's caps note, the feed's secondary ink, the review meta),
// the dim label on a card (a review's text), the status words on a card (a review's flag), the
// neutral tag on a card and the page (the measure chip there), the focus ring on a card and the
// page (rows, tiles, toggles), and every button state (the back arrow, Apply, the commit); the
// pieces' own groups for DataSheet, ToggleChip, SearchField, EmptyState, Panel and Chip.
//
// Not measured, on purpose: a tile's and a move row's resting edge (`--border-subtle`,
// `--border`) and an applied row's success edge: the fill and the words separate them (an
// applied row also says "Applied" on its button), so the edges are decoration; a section foot's
// hairline, likewise.

import type { UiContrastCase } from "../contrast.harness";

/** A flag row's grounds: a list on a card, under the pointer, the open one. */
const FLAG_ROWS: [string, string[]][] = [
  ["on a card", ["--card"]],
  ["in a hovered row", ["--row-hover", "--card"]],
  ["in a selected row", ["--row-selected", "--card"]]
];

/** A move row's grounds: offered (the card) and applied (the success tint over the card). */
const MOVE_ROWS: [string, string[]][] = [
  ["offered, on its card", ["--card"]],
  ["applied, on the success tint", ["--status-success-bg", "--card"]]
];

export const contrastCases: UiContrastCase[] = [
  // ── ReorderSheet ────────────────────────────────────────────────────────────────────────────
  {
    name: "ReorderSheet pick, checked: the brand fill on the sheet's card",
    fg: "--brand",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },

  // ── MapSidebar, MapDetailCard ───────────────────────────────────────────────────────────────
  {
    name: "MapSidebar title on the panel",
    fg: "--foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "MapTile: a row's name on the tile on the panel",
    fg: "--foreground",
    fill: "--field",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "MapTile: a row's meta line on the tile on the panel",
    fg: "--muted-foreground",
    fill: "--field",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "MapTile pressed: a row's name on the selected fill",
    fg: "--foreground",
    fill: "--segment-selected",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "MapTile pressed: a row's meta line on the selected fill",
    fg: "--muted-foreground",
    fill: "--segment-selected",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "MapSection disclosure caret on the panel",
    fg: "--muted-foreground",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },

  // ── FlagRow ─────────────────────────────────────────────────────────────────────────────────
  ...FLAG_ROWS.map(([where, on]) => ({
    name: `FlagRow rank and evidence (subtle ink), ${where}`,
    fg: "--subtle-foreground",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  ...FLAG_ROWS.slice(1).map(([where, on]) => ({
    name: `FlagRow measure chip: dim ink on the neutral tint, ${where}`,
    fg: "--dim-foreground",
    fill: "--status-neutral-bg",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  {
    name: "FlagRow name and figure, in a selected row",
    fg: "--foreground",
    on: ["--row-selected", "--card"],
    min: 4.5,
    kind: "text"
  },

  // ── CoverageRebalanceList ───────────────────────────────────────────────────────────────────
  ...MOVE_ROWS.flatMap(([where, on]) => [
    {
      name: `CoverageRebalanceList destination and result (point colour), ${where}`,
      fg: "--link",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `CoverageRebalanceList origin, ${where}`,
      fg: "--foreground",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `CoverageRebalanceList detail line, ${where}`,
      fg: "--muted-foreground",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `CoverageRebalanceList connector arrow, ${where}`,
      fg: "--muted-foreground",
      on,
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `CoverageRebalanceList Apply's focus ring, ${where}`,
      fg: "--focus-ring-color",
      on,
      min: 3,
      kind: "focus" as const
    }
  ]),

  // ── FacilityActivityFeed ────────────────────────────────────────────────────────────────────
  {
    name: "FacilityActivityFeed who and where (full ink), on a card",
    fg: "--foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  }
];
