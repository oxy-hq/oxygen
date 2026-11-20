// The figures group's contrast cases: every on, selected, hover and disabled state its pieces draw,
// light and dark (contrast.test.ts runs them). See CONVENTIONS.md for the shape.
//
// Already measured by the recipes (src/ui/contrast.cases.ts) and not repeated here: a tag's ink on
// its tint on a card and on the page (the Chip's label), the eyebrow and the note on a card and
// the page, the neutral tab counts, the focus ring on a card and the page. Measured by
// `mixContrast.test.ts` beside this file, because the harness reads tokens and these are mixes:
// the Chip ×'s hover (its own ink at 15% behind it).
//
// Not measured, on purpose: a callout's resting edge (`--status-*-line`; the tint and the caret
// say "door", the edge is decoration) and the strip's caret at half ink (decorative: the label
// says where the door leads).

import type { UiContrastCase } from "../contrast.harness";
import { APP_MARK_FADE_FLOOR } from "./AppMark";

const TONES = ["success", "warning", "error", "info"] as const;

/** Each status's ink and tint; neutral reads in the dim ink on the neutral tint. */
const INK = {
  success: { fg: "--status-success", fill: "--status-success-bg" },
  warning: { fg: "--status-warning", fill: "--status-warning-bg" },
  error: { fg: "--status-error", fill: "--status-error-bg" },
  info: { fg: "--status-info", fill: "--status-info-bg" },
  neutral: { fg: "--dim-foreground", fill: "--status-neutral-bg" }
} as const;
const ALL = [...TONES, "neutral"] as const;

/** The grounds a figure sits on beyond a card and the page. */
const ROWS: [string, string[]][] = [
  ["in a selected row", ["--row-selected", "--card"]],
  ["in a hovered row", ["--row-hover", "--card"]],
  ["in a dialog", ["--popover"]]
];
const GROUNDS: [string, string[]][] = [
  ["on a card", ["--card"]],
  ["on the page", ["--background"]],
  ...ROWS
];

const DOT = {
  success: "--dot-success",
  warning: "--dot-warning",
  error: "--dot-error",
  info: "--dot-info",
  neutral: "--status-neutral-series"
} as const;

/** Where the reference draws an app mark: a card on the home, the page (an app's header, a read
 *  notice), a dialog, and a notice row under the pointer on either. Not a selected row: none
 *  draws one there, and on dark the foot measures 2.97:1 on `--row-selected` over a card. */
const APP_MARK_GROUNDS: [string, string[]][] = [
  ["on a card", ["--card"]],
  ["on the page", ["--background"]],
  ["in a dialog", ["--popover"]],
  ["in a hovered row on a card", ["--row-hover", "--card"]],
  ["in a hovered row on the page", ["--row-hover", "--background"]]
];

export const contrastCases: UiContrastCase[] = [
  // ── Chip ──────────────────────────────────────────────────────────────────────────────────
  ...ALL.flatMap((tone) =>
    ROWS.map(([where, on]) => ({
      name: `Chip ${tone}: label on its tint, ${where}`,
      ...INK[tone],
      on,
      min: 4.5,
      kind: "text" as const
    }))
  ),
  ...ALL.map((tone) => ({
    name: `Chip ${tone}: the remove × at rest (70%), on its tint on a card`,
    fg: INK[tone].fg,
    on: [INK[tone].fill, "--card"],
    opacity: 0.7,
    min: 3,
    kind: "non-text" as const
  })),
  ...ALL.map((tone) => ({
    name: `Chip ${tone}: the remove ×, disabled (50%), on its tint on a card`,
    fg: INK[tone].fg,
    on: [INK[tone].fill, "--card"],
    opacity: 0.5,
    min: 2,
    kind: "inactive" as const
  })),
  ...ALL.map((tone) => ({
    name: `Chip ${tone}: the remove ×'s focus ring on its tint on a card`,
    fg: "--focus-ring-color",
    on: [INK[tone].fill, "--card"],
    min: 3,
    kind: "focus" as const
  })),

  // ── StatusDot ─────────────────────────────────────────────────────────────────────────────
  ...ALL.flatMap((tone) =>
    [
      ...GROUNDS,
      ["on a selected segment", ["--segment-selected"]] as [string, string[]],
      ["on an unselected segment", ["--segment-track"]] as [string, string[]]
    ].map(([where, on]) => ({
      name: `StatusDot ${tone}: the dot (and the quiet ring), ${where}`,
      fg: DOT[tone],
      on,
      min: 3,
      kind: "non-text" as const
    }))
  ),

  // ── CountBadge ────────────────────────────────────────────────────────────────────────────
  ...TONES.flatMap((tone) =>
    (
      [
        ["on a selected segment", ["--segment-selected"]],
        ["on an unselected segment", ["--segment-track"]],
        ["on a selected tab (the page)", ["--background"]],
        ["in a dialog", ["--popover"]]
      ] as [string, string[]][]
    ).map(([where, on]) => ({
      name: `CountBadge ${tone}: count on its tint, ${where}`,
      ...INK[tone],
      on,
      min: 4.5,
      kind: "text" as const
    }))
  ),
  ...GROUNDS.slice(0, 2).map(([where, on]) => ({
    name: `CountBadge neutral: count on the muted tint, ${where}`,
    fg: "--muted-foreground",
    fill: "--muted",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  {
    name: "CountBadge neutral, selected: count on its tint on the selected segment",
    fg: "--foreground",
    fill: "--segment-track",
    on: ["--segment-selected"],
    min: 4.5,
    kind: "text"
  },

  // ── AlertStrip ────────────────────────────────────────────────────────────────────────────
  ...ALL.map((tone) => ({
    name: `AlertStrip ${tone}: the alert's words on its tint, on the strip's card`,
    ...INK[tone],
    on: ["--card"],
    min: 4.5,
    kind: "text" as const
  })),
  ...ALL.map((tone) => ({
    name: `AlertStrip ${tone}: a door's edge on hover (its own ink), on the strip's card`,
    fg: INK[tone].fg,
    on: ["--card"],
    min: 3,
    kind: "non-text" as const
  })),
  {
    name: "AlertStrip: a door's focus ring on the strip's card",
    fg: "--focus-ring-color",
    on: ["--card"],
    min: 3,
    kind: "focus"
  },

  // ── Stat ──────────────────────────────────────────────────────────────────────────────────
  ...GROUNDS.slice(0, 2).flatMap(([where, on]) => [
    {
      name: `Stat boxed: the eyebrow on the tile, ${where}`,
      fg: "--subtle-foreground",
      fill: "--field",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `Stat boxed: the figure on the tile, ${where}`,
      fg: "--foreground",
      fill: "--field",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `Stat boxed: the note on the tile, ${where}`,
      fg: "--muted-foreground",
      fill: "--field",
      on,
      min: 4.5,
      kind: "text" as const
    },
    {
      name: `Stat: the figure unboxed, ${where}`,
      fg: "--foreground",
      on,
      min: 4.5,
      kind: "text" as const
    }
  ]),
  ...(["--status-success", "--status-warning", "--status-error"] as const).map((fg) => ({
    name: `Stat boxed: a judged figure (${fg}) on the tile on a card`,
    fg,
    fill: "--field",
    on: ["--card"],
    min: 4.5,
    kind: "text" as const
  })),

  // ── AsOf ──────────────────────────────────────────────────────────────────────────────────
  ...GROUNDS.map(([where, on]) => ({
    name: `AsOf behind: the warning words, ${where}`,
    fg: "--status-warning",
    on,
    min: 4.5,
    kind: "text" as const
  })),
  {
    name: "AsOf: the note in a dialog",
    fg: "--muted-foreground",
    on: ["--popover"],
    min: 4.5,
    kind: "text"
  },
  // ── AppMark (added with the patterns group, 19 Nov) ──────────────────────────────────────────
  // The glyph is a mark (1.4.11, 3:1), faded from full ink at its top to APP_MARK_FADE_FLOOR at
  // its foot. The square is folded into `on`, so `opacity` fades the ink alone: the foot is the
  // ink at the floor over the square over its ground, as the browser composites it. On dark the
  // square is translucent, so each ground is its own reading.
  ...APP_MARK_GROUNDS.flatMap(([where, on]) => [
    {
      name: `AppMark glyph top (full ink) on its square, ${where}`,
      fg: "--app-mark-ink",
      on: ["--app-mark-ground", ...on],
      min: 3,
      kind: "non-text" as const
    },
    {
      name: `AppMark glyph foot (${APP_MARK_FADE_FLOOR * 100}% ink) on its square, ${where}`,
      fg: "--app-mark-ink",
      on: ["--app-mark-ground", ...on],
      opacity: APP_MARK_FADE_FLOOR,
      min: 3,
      kind: "non-text" as const
    }
  ])
];
