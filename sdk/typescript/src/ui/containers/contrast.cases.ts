// The containers group's contrast cases: what its pieces draw that no recipe case covers, on the
// grounds they sit on, light and dark (contrast.test.ts runs them). The buttons inside (the Modal's
// X, the confirms' answers) are the `btn` recipes' cases; the eyebrow over a Panel's rule is the
// recipes' `eyebrow, on a card`; focus rings on a card or the page are `focusRing`'s.

import type { UiContrastCase } from "../contrast.harness";

const GROUNDS = [
  ["--card", "on a card"],
  ["--background", "on the page"]
] as const;

export const contrastCases: UiContrastCase[] = [
  // ── SidePanel and Modal: an opaque card over the scrimmed page ─────────────────────────────
  {
    name: "SidePanel: text on the panel's card, over the scrim",
    fg: "--foreground",
    fill: "--card",
    on: ["--scrim", "--background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "SidePanel: a muted line on the panel's card, over the scrim",
    fg: "--muted-foreground",
    fill: "--card",
    on: ["--scrim", "--background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Modal: the title on the panel's card, over the scrim",
    fg: "--foreground",
    fill: "--card",
    on: ["--scrim", "--background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Modal: the X's glyph (an icon-only control) on the card",
    fg: "--muted-foreground",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "Modal: a focus ring inside the panel",
    fg: "--focus-ring-color",
    on: ["--card"],
    min: 3,
    kind: "focus"
  },
  // ── ConfirmInline: the asking row's neutral wash ───────────────────────────────────────────
  {
    name: "ConfirmInline: the question on the asking row's wash",
    fg: "--foreground",
    on: ["--status-neutral-bg", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ConfirmInline: a cell's dim ink on the asking row's wash",
    fg: "--dim-foreground",
    on: ["--status-neutral-bg", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ConfirmInline: Keep it's label on its fill, on the wash",
    fg: "--foreground",
    fill: "--btn-surface",
    on: ["--status-neutral-bg", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ConfirmInline: Revoke it's label on the primary fill, on the wash",
    fg: "--inverse-foreground",
    fill: "--brand",
    on: ["--status-neutral-bg", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ConfirmInline: Revoke it's busy spinner on the primary fill, on the wash",
    fg: "--inverse-foreground",
    fill: "--brand",
    on: ["--status-neutral-bg", "--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "ConfirmInline: the focus ring on the wash",
    fg: "--focus-ring-color",
    on: ["--status-neutral-bg", "--card"],
    min: 3,
    kind: "focus"
  },
  // ── HoverHint: the muted bubble, and the ⓘ ─────────────────────────────────────────────────
  ...GROUNDS.map(
    ([ground, where]): UiContrastCase => ({
      name: `HoverHint: the hint on its muted bubble, ${where}`,
      fg: "--foreground",
      fill: "--muted",
      on: [ground],
      min: 4.5,
      kind: "text"
    })
  ),
  ...GROUNDS.map(
    ([ground, where]): UiContrastCase => ({
      name: `HoverHint: the ⓘ mark at rest, ${where}`,
      fg: "--muted-foreground",
      on: [ground],
      min: 3,
      kind: "non-text"
    })
  ),
  ...GROUNDS.map(
    ([ground, where]): UiContrastCase => ({
      name: `HoverHint: the ⓘ mark, hover, ${where}`,
      fg: "--foreground",
      on: [ground],
      min: 3,
      kind: "non-text"
    })
  ),
  {
    name: "HoverHint: the hint on its muted bubble, inside a panel over the scrim",
    fg: "--foreground",
    fill: "--muted",
    on: ["--card", "--scrim", "--background"],
    min: 4.5,
    kind: "text"
  }
];
