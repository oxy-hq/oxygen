// The charts group's contrast cases, light and dark (contrast.test.ts runs them): every series
// colour a chart can draw against each surface it sits on (a card, a dialog, and for a sparkline
// a hovered or selected table row), held to 1.4.11's 3:1 since a line is the information; every
// label, the readout and the toast's words as text; the crosshair as a graphic. The focus ring on
// a card is the recipes' case.
//
// Not cased: the gridlines (`--border-subtle`, a recessive guide, not information) and the area
// fill (decoration under the line, which carries the value).

import type { UiContrastCase } from "../contrast.harness";
import { seriesColour } from "./series";

/** The token a series is drawn in, read from the chart's own map, so a case measures exactly
 *  the colour the chart paints. */
const drawn = (tone: Parameters<typeof seriesColour>[0]) =>
  /^var\((--[a-z0-9-]+)\)$/.exec(seriesColour(tone))?.[1] ?? seriesColour(tone);

const SERIES: [string, string][] = [
  ["the store's line (no status)", drawn(null)],
  ["the comparison line", "--trend-peer"],
  ["success", drawn("success")],
  ["warning", drawn("warning")],
  ["error", drawn("error")],
  ["info", drawn("info")],
  ["neutral", drawn("neutral")]
];

const GROUNDS: [string, string[]][] = [
  ["on a card", ["--card"]],
  ["on a dialog", ["--popover"]],
  ["in a hovered row", ["--row-hover", "--card"]],
  ["in a selected row", ["--row-selected", "--card"]]
];

const text = (name: string, fg: string, on: string[], fill?: string): UiContrastCase => ({
  name,
  fg,
  fill,
  on,
  min: 4.5,
  kind: "text"
});

export const contrastCases: UiContrastCase[] = [
  ...SERIES.flatMap(([series, fg]) =>
    GROUNDS.map(
      ([where, on]): UiContrastCase => ({
        name: `Chart series, ${series}, ${where}`,
        fg,
        on,
        min: 3,
        kind: "non-text"
      })
    )
  ),
  text("Sparkline: the dash for no history, on a card", "--muted-foreground", ["--card"]),
  text("TrendChart: axis ticks and months, on a card", "--subtle-foreground", ["--card"]),
  text("TrendChart: axis ticks and months, on a dialog", "--subtle-foreground", ["--popover"]),
  text("TrendChart: the subject's end label, on a card", "--foreground", ["--card"]),
  text("TrendChart: the comparison's end label, on a card", "--muted-foreground", ["--card"]),
  text("TrendChart: the phone readout, on a card", "--dim-foreground", ["--card"]),
  text("TrendChart: the phone readout's prompt, on a card", "--muted-foreground", ["--card"]),
  text("TrendChart: toast month and value", "--foreground", ["--card"], "--muted"),
  text("TrendChart: toast notes", "--muted-foreground", ["--card"], "--muted"),
  {
    name: "TrendChart: the toast's edge against a card",
    fg: "--border",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "TrendChart: the crosshair, on a card",
    fg: "--border-strong",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  }
];
