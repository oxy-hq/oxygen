// The tables group's contrast cases: every ink, mark and ring its pieces draw, in each state, on
// each ground the piece sits on (a card, a dialog's popover, a hovered or selected row, the muted
// child-row and totals fill), light and dark (contrast.test.ts runs them). The cell's dim ink on a
// card, hovered and selected, and the focus ring on a card and in a selected row, are the recipe
// cases (src/ui/contrast.cases.ts) and are not repeated here.
//
// Not cased, on purpose: the sort arrow's hover glimpse at 40% (decoration; the state is in
// `aria-sort` and the title), the sticky fade in TableScroll and a folded DataSheet column (no
// ink), and row rules (separators, not controls).

import type { UiContrastCase } from "../contrast.harness";

const CARD = ["--card"];
const ROW_HOVER = ["--row-hover", "--card"];
const ROW_SELECTED = ["--row-selected", "--card"];
const MUTED = ["--muted", "--card"];

const text = (name: string, fg: string, on: string[], fill?: string): UiContrastCase => ({
  name,
  fg,
  fill,
  on,
  min: 4.5,
  kind: "text"
});
const mark = (name: string, fg: string, on: string[]): UiContrastCase => ({
  name,
  fg,
  on,
  min: 3,
  kind: "non-text"
});
const ring = (name: string, on: string[]): UiContrastCase => ({
  name,
  fg: "--focus-ring-color",
  on,
  min: 3,
  kind: "focus"
});

const ROW_GROUNDS: [string, string[]][] = [
  ["on a card", CARD],
  ["in a hovered row", ROW_HOVER],
  ["in a selected row", ROW_SELECTED]
];

export const contrastCases: UiContrastCase[] = [
  // ── SortHeader ──────────────────────────────────────────────────────────────────────────
  text("SortHeader: heading at rest, on a card", "--subtle-foreground", CARD),
  text("SortHeader: heading at rest, on a dialog", "--subtle-foreground", ["--popover"]),
  text("SortHeader: sorted or hovered heading, on a card", "--foreground", CARD),
  mark("SortHeader: the sorted column's arrow, on a card", "--foreground", CARD),
  ring("SortHeader: focus ring on its button, on a dialog", ["--popover"]),

  // ── MetricCell ──────────────────────────────────────────────────────────────────────────
  ...ROW_GROUNDS.flatMap(([where, on]) => [
    text(
      `MetricCell warning: figure on its tint, ${where}`,
      "--status-warning",
      on,
      "--status-warning-bg"
    ),
    text(
      `MetricCell error: figure on its tint, ${where}`,
      "--status-error",
      on,
      "--status-error-bg"
    ),
    text(
      `MetricCell quiet: figure on the warning tint at weight 400, ${where}`,
      "--status-warning",
      on,
      "--status-warning-bg"
    ),
    text(`MetricCell notable: figure, ${where}`, "--foreground", on),
    text(`MetricCell provisional: figure and asterisk, ${where}`, "--subtle-foreground", on),
    text(`MetricCell: comparison, label and as-of, ${where}`, "--subtle-foreground", on)
  ]),

  // ── PanelTable ──────────────────────────────────────────────────────────────────────────
  text("PanelTable: column heading, on a card", "--muted-foreground", CARD),
  text("PanelTable: column heading, on a dialog", "--muted-foreground", ["--popover"]),
  text("PanelTable: row name, on a card", "--foreground", CARD),
  text("PanelTable: row name, on a dialog", "--foreground", ["--popover"]),
  text("PanelTable: cell, on a dialog", "--dim-foreground", ["--popover"]),
  text("PanelTable: row name in a hovered row", "--foreground", ROW_HOVER),
  text("PanelTable: row name in the selected or open row", "--foreground", ROW_SELECTED),
  mark("PanelTable: expand caret, on a card", "--muted-foreground", CARD),
  mark("PanelTable: expand caret, in a hovered row", "--muted-foreground", ROW_HOVER),
  mark("PanelTable: expand caret, in the open row", "--muted-foreground", ROW_SELECTED),
  ring("PanelTable: focus ring on a row's name, in a hovered row", ROW_HOVER),
  text("PanelTable: child row's name and cells, on the muted fill", "--dim-foreground", MUTED),
  text("PanelTable: totals row, on the muted fill", "--foreground", MUTED),
  text("PanelTable: lead and footnote, on a card", "--muted-foreground", CARD),

  // ── LinkList ────────────────────────────────────────────────────────────────────────────
  text("LinkList: row text, on a card", "--dim-foreground", CARD),
  text("LinkList: row text, hovered", "--dim-foreground", ROW_HOVER),
  text("LinkList: the current row's text", "--dim-foreground", ROW_SELECTED),
  text("LinkList: a column in full ink, the current row", "--foreground", ROW_SELECTED),
  ring("LinkList: inset focus ring, on a card", CARD),
  ring("LinkList: inset focus ring, the current row", ROW_SELECTED),

  // ── TableScroll ─────────────────────────────────────────────────────────────────────────
  ring("TableScroll: inset focus ring on the box, on a card", CARD),
  ring("TableScroll: inset focus ring on the box, on the page", ["--background"]),

  // ── DataSheet ───────────────────────────────────────────────────────────────────────────
  text("DataSheet: band and column headings, on a card", "--muted-foreground", CARD),
  text("DataSheet: row name, on a card", "--foreground", CARD),
  text("DataSheet: folded group's stub name, on a card", "--muted-foreground", CARD),
  mark("DataSheet: row stripe, success", "--status-success", CARD),
  mark("DataSheet: row stripe, warning", "--status-warning", CARD),
  mark("DataSheet: row stripe, error", "--status-error", CARD),
  mark("DataSheet: row stripe, info", "--status-info", CARD),
  ring("DataSheet: focus ring on the box, on the page", ["--background"])
];
