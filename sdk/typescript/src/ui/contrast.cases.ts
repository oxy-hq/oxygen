// The recipes' contrast cases (base.css): every on, selected, hover and disabled state the
// recipes draw, on the grounds the reference draws them on. A group's own pieces add theirs in
// `<group>/contrast.cases.ts`. Measured by contrast.test.ts in every direction, both modes.

import type { UiContrastCase } from "./contrast.harness";

const GROUNDS = [["--card"], ["--background"]] as const;
const onEach = (make: (on: string[], where: string) => UiContrastCase): UiContrastCase[] =>
  GROUNDS.map((g) => make([...g], g[0] === "--card" ? "on a card" : "on the page"));

export const recipeCases: UiContrastCase[] = [
  // ── Focus ───────────────────────────────────────────────────────────────────────────────
  ...onEach((on, where) => ({
    name: `focusRing, ${where}`,
    fg: "--focus-ring-color",
    on,
    min: 3,
    kind: "focus"
  })),
  {
    name: "focusRing, in a selected row",
    fg: "--focus-ring-color",
    on: ["--row-selected", "--card"],
    min: 3,
    kind: "focus"
  },
  {
    name: "focusRingInset, on the selected segment",
    fg: "--focus-ring-color",
    on: ["--segment-selected"],
    min: 3,
    kind: "focus"
  },
  {
    name: "focusRingInset, on an unselected segment",
    fg: "--focus-ring-color",
    on: ["--segment-track"],
    min: 3,
    kind: "focus"
  },

  // ── Buttons ─────────────────────────────────────────────────────────────────────────────
  ...onEach((on, where) => ({
    name: `btn primary: label on its fill, ${where}`,
    fg: "--inverse-foreground",
    fill: "--brand",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn primary: label on its fill, hover, ${where}`,
    fg: "--inverse-foreground",
    fill: "--brand-hover",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn primary: label, disabled, ${where}`,
    fg: "--inverse-foreground",
    fill: "--brand",
    on,
    opacity: 0.5,
    min: 2,
    kind: "inactive"
  })),
  ...onEach((on, where) => ({
    name: `btn secondary: label on its fill, ${where}`,
    fg: "--foreground",
    fill: "--btn-surface",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn secondary: label on its fill, hover, ${where}`,
    fg: "--foreground",
    fill: "--btn-surface-hover",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn secondary: label, disabled, ${where}`,
    fg: "--foreground",
    fill: "--btn-surface",
    on,
    opacity: 0.5,
    min: 2,
    kind: "inactive"
  })),
  ...onEach((on, where) => ({
    name: `btn text: label, ${where}`,
    fg: "--dim-foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn text: label, hover, ${where}`,
    fg: "--foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `btn text: label, disabled, ${where}`,
    fg: "--dim-foreground",
    on,
    opacity: 0.5,
    min: 2,
    kind: "inactive"
  })),

  // ── Fields and selects ──────────────────────────────────────────────────────────────────
  {
    name: "field: value on the field",
    fg: "--foreground",
    on: ["--field"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "field: placeholder on the field",
    fg: "--muted-foreground",
    on: ["--field"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "select: placeholder sentinel on the field",
    fg: "--muted-foreground",
    on: ["--field"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "field: resting edge against the field",
    fg: "--field-border",
    on: ["--field"],
    min: 3,
    kind: "control-border"
  },
  ...onEach((on, where) => ({
    name: `field: resting edge, ${where}`,
    fg: "--field-border",
    on,
    min: 3,
    kind: "control-border"
  })),
  {
    name: "field: focused edge against the field",
    fg: "--focus-ring-color",
    on: ["--field"],
    min: 3,
    kind: "control-border"
  },
  {
    name: "field: error edge against the field",
    fg: "--status-error",
    on: ["--field"],
    min: 3,
    kind: "control-border"
  },
  {
    name: "field: value, disabled",
    fg: "--disabled-foreground",
    on: ["--muted"],
    min: 2,
    kind: "inactive"
  },
  {
    name: "field quiet: placeholder at rest, on a card",
    fg: "--muted-foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },

  // ── Status: tags, callouts, ink ─────────────────────────────────────────────────────────
  ...(["success", "warning", "error", "info"] as const).flatMap((tone) =>
    onEach((on, where) => ({
      name: `tag ${tone}: ink on its tint, ${where}`,
      fg: `--status-${tone}`,
      fill: `--status-${tone}-bg`,
      on,
      min: 4.5,
      kind: "text" as const
    }))
  ),
  ...onEach((on, where) => ({
    name: `tag neutral: dim ink on the neutral tint, ${where}`,
    fg: "--dim-foreground",
    fill: "--status-neutral-bg",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...(["success", "warning", "error", "info"] as const).map((tone) => ({
    name: `toneText ${tone}: a status word on a card`,
    fg: `--status-${tone}`,
    on: ["--card"],
    min: 4.5,
    kind: "text" as const
  })),

  // ── Type roles ──────────────────────────────────────────────────────────────────────────
  ...onEach((on, where) => ({
    name: `eyebrow, ${where}`,
    fg: "--subtle-foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  {
    name: "eyebrow, in a selected row",
    fg: "--subtle-foreground",
    on: ["--row-selected", "--card"],
    min: 4.5,
    kind: "text"
  },
  ...onEach((on, where) => ({
    name: `note and caption, ${where}`,
    fg: "--muted-foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  {
    name: "label (dim) on a card",
    fg: "--dim-foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },

  // ── Rows and cells ──────────────────────────────────────────────────────────────────────
  {
    name: "cell: dim ink on a card",
    fg: "--dim-foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "cell: dim ink in a hovered row",
    fg: "--dim-foreground",
    on: ["--row-hover", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "cell: dim ink in a selected row",
    fg: "--dim-foreground",
    on: ["--row-selected", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "cell: a row's name in a selected row",
    fg: "--foreground",
    on: ["--row-selected", "--card"],
    min: 4.5,
    kind: "text"
  },

  // ── Tabs, segments, counts, section links ───────────────────────────────────────────────
  {
    name: "tab on: label on the page it opens into",
    fg: "--foreground",
    on: ["--background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "tab off: label on the strip (pageTabs)",
    fg: "--dim-foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "tab off: label, hover",
    fg: "--foreground",
    on: ["--row-hover", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "top-bar tab on: the underline against the bar",
    fg: "--foreground",
    on: ["--topbar-bg"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "top-bar tab off: label on the bar",
    fg: "--dim-foreground",
    on: ["--topbar-bg"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "segment on: label on the selected fill",
    fg: "--foreground",
    on: ["--segment-selected"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "segment off: label on the track",
    fg: "--dim-foreground",
    on: ["--segment-track"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "segment off: label, hover",
    fg: "--foreground",
    on: ["--row-hover", "--segment-track"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "tabCountNeutral: count on its tint",
    fg: "--muted-foreground",
    on: ["--muted"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "tabCountNeutralOn: count on a selected segment",
    fg: "--foreground",
    on: ["--segment-track"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "section link on: label on the selected fill",
    fg: "--foreground",
    on: ["--row-selected", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "section link off: label",
    fg: "--dim-foreground",
    on: ["--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "section link off: label, hover",
    fg: "--foreground",
    on: ["--row-hover", "--card"],
    min: 4.5,
    kind: "text"
  }
];
