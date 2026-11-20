// The feedback group's contrast cases: what its pieces draw, on the grounds they sit on (a card, the
// page, a side panel's card over the scrim), light and dark (contrast.test.ts runs them). Try
// again and the empty state's actions are Buttons: the `btn` recipes' cases.

import type { UiContrastCase } from "../contrast.harness";

const GROUNDS = [
  [["--card"], "on a card"],
  [["--background"], "on the page"],
  [["--card", "--scrim", "--background"], "in a panel over the scrim"]
] as const;

const onEach = (make: (on: string[], where: string) => UiContrastCase): UiContrastCase[] =>
  GROUNDS.map(([on, where]) => make([...on], where));

export const contrastCases: UiContrastCase[] = [
  // ── LoadError: the error and warning callouts ──────────────────────────────────────────────
  ...onEach((on, where) => ({
    name: `LoadError error: the message on its tint, ${where}`,
    fg: "--status-error",
    fill: "--status-error-bg",
    on,
    min: 4.5,
    kind: "text"
  })),
  ...onEach((on, where) => ({
    name: `LoadError warning: the message on its tint, ${where}`,
    fg: "--status-warning",
    fill: "--status-warning-bg",
    on,
    min: 4.5,
    kind: "text"
  })),
  {
    name: "LoadError: Try again's busy spinner on its secondary fill",
    fg: "--foreground",
    fill: "--btn-surface",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  // ── Loading: the line, the bar's fill against its track ────────────────────────────────────
  ...onEach((on, where) => ({
    name: `Loading: the status line, ${where}`,
    fg: "--muted-foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  {
    name: "LoadingBar: the fill against its track",
    fg: "--brand",
    on: ["--muted", "--card"],
    min: 3,
    kind: "non-text"
  },
  // ── EmptyState: the claim, and the icon in its tile ────────────────────────────────────────
  ...onEach((on, where) => ({
    name: `EmptyState: the claim, ${where}`,
    fg: "--foreground",
    on,
    min: 4.5,
    kind: "text"
  })),
  {
    name: "EmptyState: the icon on its muted tile",
    fg: "--muted-foreground",
    fill: "--muted",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  }
];
