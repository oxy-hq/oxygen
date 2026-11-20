// The navigation group's contrast cases: every on, selected, hover and focus state its pieces draw
// beyond the recipes' own cases (src/ui/contrast.cases.ts: tab, top-bar tab, segment, section
// link, the neutral counts), light and dark (contrast.test.ts runs them). See CONVENTIONS.md.
//
// Grounds are where each piece sits: a strip's segments on its track, the selected one on its
// fill over the track; the top-bar strip on the bar; the More menu on the popover; the Sections
// column on the sidebar. (PhoneOptionSheet, the shell's, sits on the kit's mixed surface, which is
// no theme token: its cases are shell/phone/phoneOptionSheetContrast.test.ts.)

import type { UiContrastCase } from "../contrast.harness";
import type { StatusTone } from "../tone";

/** The tones a count wears (DefaultTabs draws success and info neutral, decision 31). */
const TONES: Extract<StatusTone, "warning" | "error">[] = ["warning", "error"];

/** A toned count (`chipTone`) on the three grounds a strip draws it on. */
const countCases: UiContrastCase[] = TONES.flatMap((tone) => [
  {
    name: `DefaultTabs count ${tone}: on a selected segment`,
    fg: `--status-${tone}`,
    fill: `--status-${tone}-bg`,
    on: ["--segment-selected", "--segment-track"],
    min: 4.5,
    kind: "text" as const
  },
  {
    name: `DefaultTabs count ${tone}: on an unselected segment`,
    fg: `--status-${tone}`,
    fill: `--status-${tone}-bg`,
    on: ["--segment-track"],
    min: 4.5,
    kind: "text" as const
  },
  {
    name: `DefaultTabs count ${tone}: on an unselected segment, hover`,
    fg: `--status-${tone}`,
    fill: `--status-${tone}-bg`,
    on: ["--row-hover", "--segment-track"],
    min: 4.5,
    kind: "text" as const
  }
]);

export const contrastCases: UiContrastCase[] = [
  ...countCases,
  {
    name: "DefaultTabs count neutral: on an unselected segment",
    fg: "--muted-foreground",
    fill: "--muted",
    on: ["--segment-track"],
    min: 4.5,
    kind: "text"
  },

  // ── Segmented ──────────────────────────────────────────────────────────────────────────────
  // The selected state the BMG review caught (18 Nov, Supplier Ordering, 1.08:1 in dark): an
  // app's light palette leaked `--segment-selected` under the dark ink. segmentedContrast.test.ts
  // proves this case fails on those colours.
  {
    name: "Segmented selected: label on the selected fill, on the track, on a card",
    fg: "--foreground",
    fill: "--segment-selected",
    on: ["--segment-track", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Segmented selected: label on the selected fill, on the track, on the page",
    fg: "--foreground",
    fill: "--segment-selected",
    on: ["--segment-track", "--background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Segmented unselected: label on the track, on a card",
    fg: "--dim-foreground",
    on: ["--segment-track", "--card"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Segmented unselected: label, hover",
    fg: "--foreground",
    on: ["--row-hover", "--segment-track", "--card"],
    min: 4.5,
    kind: "text"
  },

  // ── ParentTabs in the top bar, and its More menu ───────────────────────────────────────────
  {
    name: "ParentTabs top bar: tab, hover",
    fg: "--foreground",
    on: ["--topbar-bg"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ParentTabs top bar: the focus ring inside a tab",
    fg: "--focus-ring-color",
    on: ["--topbar-bg"],
    min: 3,
    kind: "focus"
  },
  {
    name: "ParentTabs More: the dots, closed",
    fg: "--dim-foreground",
    on: ["--topbar-bg"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "ParentTabs More: the dots, open",
    fg: "--foreground",
    on: ["--topbar-bg"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "ParentTabs More: a hidden screen's count",
    fg: "--muted-foreground",
    fill: "--muted",
    on: ["--topbar-bg"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ParentTabs menu: a screen",
    fg: "--dim-foreground",
    on: ["--popover"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ParentTabs menu: a screen, hover",
    fg: "--foreground",
    on: ["--row-hover", "--popover"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "ParentTabs menu: the focus ring inside a screen",
    fg: "--focus-ring-color",
    on: ["--popover"],
    min: 3,
    kind: "focus"
  },
  {
    name: "ParentTabs page strip: the focus ring on the strip",
    fg: "--focus-ring-color",
    on: ["--card"],
    min: 3,
    kind: "focus"
  },

  // ── Subnav, on the sidebar ─────────────────────────────────────────────────────────────────
  {
    name: "Subnav link: label on the sidebar",
    fg: "--dim-foreground",
    on: ["--sidebar-background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Subnav link: label, hover",
    fg: "--foreground",
    on: ["--row-hover", "--sidebar-background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Subnav link current: label on the selected fill",
    fg: "--foreground",
    on: ["--row-selected", "--sidebar-background"],
    min: 4.5,
    kind: "text"
  },
  {
    name: "Subnav link: the focus ring on the sidebar",
    fg: "--focus-ring-color",
    on: ["--sidebar-background"],
    min: 3,
    kind: "focus"
  },
  {
    name: "Subnav link current: the focus ring on the selected fill",
    fg: "--focus-ring-color",
    on: ["--row-selected", "--sidebar-background"],
    min: 3,
    kind: "focus"
  },

  // ── TopBarLeft ─────────────────────────────────────────────────────────────────────────────
  {
    name: "TopBarLeft stand-in: the app's name on the bar",
    fg: "--foreground",
    on: ["--topbar-bg"],
    min: 4.5,
    kind: "text"
  }
];
