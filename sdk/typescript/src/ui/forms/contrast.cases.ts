// The forms group's contrast cases: every on, selected, hover and disabled state its pieces draw,
// light and dark (contrast.test.ts runs them). See CONVENTIONS.md for the shape.
//
// Already measured by the recipes (src/ui/contrast.cases.ts) and not repeated: the field's value,
// placeholder, resting, focused and error edges and its disabled value; the select's sentinel; the
// label and the note on a card; the buttons; the focus ring on a card and the page. Measured by
// `mixContrast.test.ts` beside this file, because it is a mix the harness cannot name: the pressed
// ToggleChip's hover (`--link` at 10% behind its ink).

import type { UiContrastCase } from "../contrast.harness";

const PAGE: [string, string[]][] = [
  ["on a card", ["--card"]],
  ["on the page", ["--background"]]
];
const GROUNDS: [string, string[]][] = [...PAGE, ["in a dialog", ["--popover"]]];

export const contrastCases: UiContrastCase[] = [
  // ── Field ─────────────────────────────────────────────────────────────────────────────────
  ...GROUNDS.slice(1).map(([where, on]) => ({
    name: `Field: the label, ${where}`,
    fg: "--dim-foreground",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  ...GROUNDS.map(([where, on]) => ({
    name: `Field and StringListEditor: the error, ${where}`,
    fg: "--status-error",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  {
    name: "Field: the hint (note) in a dialog",
    fg: "--muted-foreground",
    on: ["--popover"],
    min: 4.5,
    kind: "text"
  },

  // ── SelectWithOther ───────────────────────────────────────────────────────────────────────
  {
    name: "SelectWithOther: an option in full ink on the field",
    fg: "--foreground",
    fill: "--field",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },

  // ── AttachmentsField ──────────────────────────────────────────────────────────────────────
  {
    name: "AttachmentsField: a tile's name on the tile",
    fg: "--foreground",
    fill: "--muted",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "AttachmentsField: a tile's size (caption) on the tile",
    fg: "--muted-foreground",
    fill: "--muted",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "AttachmentsField: a tile's failure reason on the tile",
    fg: "--status-error",
    fill: "--muted",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "AttachmentsField: the file glyph on the tile",
    fg: "--muted-foreground",
    fill: "--muted",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "AttachmentsField: Uploading on its card chip",
    fg: "--foreground",
    fill: "--card",
    on: ["--scrim", "--muted"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "AttachmentsField: Retry on the failed veil (the error tint over the card)",
    fg: "--status-error",
    fill: "--status-error-bg",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "AttachmentsField: Retry's focus ring, inside the failed veil",
    fg: "--focus-ring-color",
    on: ["--status-error-bg", "--card"],
    min: 3,
    kind: "focus"
  },
  {
    name: "AttachmentsField: the remove × on its plate",
    fg: "--foreground",
    fill: "--card",
    on: ["--muted"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "AttachmentsField: the remove × on its plate, hover",
    fg: "--foreground",
    fill: "--muted",
    on: ["--muted"],
    min: 3,
    kind: "non-text"
  },
  ...PAGE.flatMap(([where, on]) => [
    {
      name: `AttachmentsField: the picker's words on the field, ${where}`,
      fg: "--dim-foreground",
      fill: "--field",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `AttachmentsField: the picker's words, hover, ${where}`,
      fg: "--foreground",
      fill: "--row-hover",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `AttachmentsField: the picker at its limit or locked (50%), ${where}`,
      fg: "--dim-foreground",
      fill: "--field",
      on,
      opacity: 0.5,
      min: 2,
      kind: "inactive" as const
    }
  ]),

  // ── StringListEditor ──────────────────────────────────────────────────────────────────────
  ...GROUNDS.slice(1).map(([where, on]) => ({
    name: `StringListEditor: Unsaved changes, ${where}`,
    fg: "--status-warning",
    on,
    min: 4.5,
    kind: "text" as const
  })),

  // ── ToggleChip ────────────────────────────────────────────────────────────────────────────
  ...PAGE.flatMap(([where, on]) => [
    {
      name: `ToggleChip off: label on its card fill, ${where}`,
      fg: "--dim-foreground",
      fill: "--card",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `ToggleChip off: its field edge, ${where}`,
      fg: "--field-border",
      on,
      min: 3,
      kind: "control-border" as const
    },
    {
      name: `ToggleChip off: label, hover, ${where}`,
      fg: "--foreground",
      fill: "--row-hover",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `ToggleChip on: the blue label on its card fill, ${where}`,
      fg: "--link",
      fill: "--card",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `ToggleChip on: the blue edge, ${where}`,
      fg: "--link",
      on,
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `ToggleChip off, disabled (50%), ${where}`,
      fg: "--dim-foreground",
      fill: "--card",
      on,
      opacity: 0.5,
      min: 2,
      kind: "inactive" as const
    },
    {
      name: `ToggleChip on, disabled (50%), ${where}`,
      fg: "--link",
      fill: "--card",
      on,
      opacity: 0.5,
      min: 2,
      kind: "inactive" as const
    }
  ]),

  // ── SearchField ───────────────────────────────────────────────────────────────────────────
  ...PAGE.flatMap(([where, on]) => [
    {
      name: `SearchField: the magnifier on the field, ${where}`,
      fg: "--muted-foreground",
      fill: "--field",
      on,
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `SearchField quiet at rest: the magnifier with no field behind it, ${where}`,
      fg: "--muted-foreground",
      on,
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `SearchField quiet at rest: the placeholder, ${where}`,
      fg: "--muted-foreground",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `SearchField: the clear × on the field, ${where}`,
      fg: "--muted-foreground",
      fill: "--field",
      on,
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `SearchField: the clear ×, hover, ${where}`,
      fg: "--foreground",
      fill: "--row-hover",
      on: ["--field", ...on],
      min: 3,
      kind: "non-text" as const
    }
  ])
];
