// The actions group's contrast cases. The button's fills, labels, hover and disabled states are
// the `btn` recipes' (src/ui/contrast.cases.ts); what only the component draws is the busy
// spinner, a graphic in the label's ink on the button's own fill (1.4.11, 3:1). CopyButton adds
// its status words and the check that replaces its icon once copied.

import type { UiContrastCase } from "../contrast.harness";

export const contrastCases: UiContrastCase[] = [
  {
    name: "Button primary busy: the spinner on its fill",
    fg: "--inverse-foreground",
    fill: "--brand",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "Button secondary busy: the spinner on its fill",
    fg: "--foreground",
    fill: "--btn-surface",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "Button text busy: the spinner on the page",
    fg: "--dim-foreground",
    on: ["--background"],
    min: 3,
    kind: "non-text"
  },
  // CopyButton: its status ("Copied", or the refusal in words) is the note recipe beside the
  // button, wherever a copy button sits: a card, the page, a dialog or a menu (the popover).
  ...(
    [
      ["--card", "on a card"],
      ["--background", "on the page"],
      ["--popover", "in a dialog"]
    ] as const
  ).map(
    ([ground, where]): UiContrastCase => ({
      name: `CopyButton status: the words beside the button, ${where}`,
      fg: "--muted-foreground",
      on: [ground],
      min: 4.5,
      kind: "text"
    })
  ),
  {
    name: "CopyButton secondary: the check on its fill, once copied",
    fg: "--foreground",
    fill: "--btn-surface",
    on: ["--card"],
    min: 3,
    kind: "non-text"
  },
  {
    name: "CopyButton text: the check on the page, once copied",
    fg: "--dim-foreground",
    on: ["--background"],
    min: 3,
    kind: "non-text"
  }
];
