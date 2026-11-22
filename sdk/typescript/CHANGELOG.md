# Changelog

All notable changes to the Oxy TypeScript SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.32.0] - 2026-11-22

### Added

The pieces the apps kept local after 2.31.1, chosen from pictures (decision 99). Each is opt-in:
an app that passes none of the new props renders exactly as on 2.31.1
(`src/ui/keptPieces.defaults.test.tsx` holds the 2.31.1 markup of every one, desk and phone).

- **`PanelTable` `phoneCard` and `rowProps`** (Quadrogen's `PhoneRows` and its per-row hooks).
  - `phoneCard={{ title, aside, subtitle, figures }}`: on a phone layout each row is a card
    instead of a table row. The card is the tile (`--field` fill, `--border-subtle` edge,
    `--radius-surface`) at `paddingSm`; the title column first, the `aside` column (a status)
    at the title line's right end with the chevron after it, on the title's baseline; the
    `subtitle` on its own line; then the figures, label over value in the dim ink, as many to a
    line as fit at 5.5rem, with no cap. A column's new `phoneWide` takes the card's whole width
    (a sentence-long value). A row that opens (`onRowOpen`, or `rowProps().onClick`) is pressed
    anywhere on the card; its title is the button the keyboard reaches (`aria-current` when
    `isSelected`). `actions` sit at the card's foot; a card draws no totals row or sub-rows.
  - `rowProps={(row) => ({ ... })}`: `data-*`, `title`, `tabIndex`, key and click handlers on
    the desk row, after the table's own; its `className` joins `rowClassName`; its `onClick` is
    the row's when the table does not open rows itself. On a card the `data-*`, `aria-*`,
    `className` and `onClick` go on the card; `title`, `tabIndex` and `onKeyDown` stay desk-only.
  - Types `PanelPhoneCard` and `PanelRowProps`.
- **`Subnav` `showHeadings` and `header`** (Quadrogen's lab names and plant line): each
  section's heading printed over its links, and a line at the column's head, both in the
  eyebrow, inset to the links' 10px, at the SDK's 220px column and 36px rows. The printed
  heading is `aria-hidden` (the group already has the name); the header is the desk column's
  only (not in the sideways row below 768px).
- **Figure grids on a phone.** `StatGrid` `phonePairs`: on a phone layout two tiles to a row
  whatever `min` is, an odd last tile across the row (a tile wrapped in its hint too).
  `PhoneKpiStrip` `oddLastAcross` (`@oxy-hq/sdk/shell`): upright, an odd last tile across the
  row (Site Scout's KPI row). Both opt-in, because callers with an odd count exist.
- **`AlertStrip` `hint` and the `check` tone** (bmg reorder-sheet's attention row). `hint` puts
  the alert in the SDK's `HoverHint`, opened on hover and on keyboard focus; a plain callout with
  a hint becomes a Tab stop. `tone: "check"` ("look at this before acting") draws the new check
  status. `AlertStripTone` is `StatusTone | "check"`; `StatusTone` itself is unchanged, so a
  caller's `Record<StatusTone, ...>` still type-checks.
- **Theme: the check status**, `--status-check`, `--status-check-bg`, `--status-check-line`
  (and `--color-status-check*` in `tailwind.css`), beside the other status tones. Light: the
  chart purple `#7e22ce`, tint at 0.12, edge at 0.3. Dark: the ink `#c084fc` (the chart purple
  `#a855f7` holds only 3.78:1 on its own tint on the card), tint and edge from `#a855f7` at 0.14
  and 0.32. Five new contrast obligations: the word plain and on its tint on the canvas and a
  card (light 6.47 / 6.98 / 5.32 / 5.72, dark 7.49 / 6.59 / 6.62 / 5.65), and the body ink on
  the tint (14.52 light, 12.75 dark). The web-app's copy and the mobile token files are
  regenerated.
- **`Sparkline` `domain`, `band`, `reference`, `baseline` and `endTone`** (Accounts Insight's
  shared scale, four-week mean, cell foot and direction dot; Warehouse Management's safe
  temperature band). `domain` fixes the y scale; `band={[low, high]}` shades a range with dashed
  edges and `reference` draws a dashed line at a value, both in the neutral comparison grey
  (`--trend-peer`); `reference={{ value, from }}` starts the line at a point (half a step before
  it); `baseline` rules the scale's floor; `endTone` colours the end mark by a status's series
  colour, apart from the line's. Type `SparkReference`.

## [2.31.1] - 2026-11-21

### Fixed

- **A `Stat`'s `tip` taps on a phone with no frame from the app.** The tip took the phone kit's
  tap ⓘ after the label only where the app wrapped its phone layout in `TapHintFrame` (Poke
  House Bookkeeping alone); every other app's phone drew the desk ⓘ, and a tap on it closed the
  hint rather than opening it. `OxyShell` now provides the frame on a phone layout
  (`usePhoneLayout()`), so every app inside the shell gets the tap form. An app's own frame,
  inside the shell or around it, still wins; the desk is unchanged, a touch tablet's or a
  kiosk's desk layout included; a server render draws the desk. On a phone, `useTippedMin`
  now widens a tipped tile's `min` by the ⓘ in these apps too.
- **A `TrendChart` y-axis never prints the same label twice.** Its three gridlines are labelled
  in whole percents (a rating: tenths), so a scale under two of those steps printed one label
  twice ("24%", "25%", "25%"). Where two would match, all three take the readout's decimal
  ("24.2%", "24.8%", "25.4%"), and a percent chart's plot starts 12 units further in to make
  room for them; a scale under 0.4 points (0.04 stars) is widened to that around its middle,
  from zero for data that is not below it. A chart whose labels already differed draws exactly
  as before.
- **`checkSdkUiCopies` no longer reports a local that only holds a component**
  (`const Glyph = app.glyph`, `const Icon = Icons[kind]`), exported or not. It reports
  definitions only: function and class declarations, a binding whose value is a function or
  class expression, an arrow, `forwardRef(...)` or `memo(...)`, and exports of those. A binding
  of a value that is no component (`const Chip = 1`) is no longer reported either.

## [2.31.0] - 2026-11-20

### Added

- **The desk UI gaps the apps met moving onto 2.30.0**, so their local copies can go. Each is
  opt-in unless Changed below says otherwise: a piece that passes none of it is drawn as before.
  - `Panel`: `header`, an optional `title`, `headingLevel`, `as` (`section` / `aside`) with
    `label`, a fold (`foldable`, `defaultOpen`, `open`, `onOpenChange`, `right` as a function of
    the fold), `density="compact"` and `bodyClassName`.
  - `Sparkline`: `width` / `height`, `fluid`, one series (`peerPct` optional, `SparkPoint`,
    `sparkPoints`), `area`, `color`, `x` (points placed by a key), and `readout` (a Tab stop whose
    readings, by the arrow keys, Home / End or the pointer, show in a tooltip).
  - `StatusDot` `wordHidden`: the dot alone where the words beside it say the state; the word
    stays for assistive tech and as the hover title.
  - `DefaultTabs`: `disabled` options (passed over by the arrows, no click, focus kept),
    `stretch`, a `hint` per tab, node labels with `text` for the select.
  - `topBarFrom` `sm` (640px), `lg` (1024px) and `2xl` (1440px) for `ParentTabs` and
    `TopBarTabs`.
  - `HoverHint` `focusable`; `Stat` `tipMark` (a keyboard-reachable ⓘ on a desk, on by default
    for a tile with a `tip`: see Changed) and `valueAt`; `Stat` and `StatGrid` take the div's
    own props; `SidePanel` `id`.
  - `MetricCell` `quiet` step (decision 93): the warning tint and ink at weight 400, a quieter
    warning beside `warning` (600), `error`, `notable` and `clear`, with a `quiet` threshold
    between `notable` and `warning`. Two warning tiers map onto it: Watchlist's notable ->
    `quiet`, elevated -> `warning`, severe -> `error`.
  - `useModalFocus(ref, open, { auto: true })` for a dialog an app draws itself.
  - `TryAgainButton`; Try again carries `data-oxy-try-again`. `LoadingBar`'s `progress` defaults
    to one read. `Button` keeps a caller's `aria-busy`. `InfoIcon`.
  - `useOxyMode()` and `readOxyMode()` in `@oxy-hq/sdk/shell`: the light/dark mode in force.
  - tailwind.css: `--color-sidebar`, `--color-sidebar-border`, `--color-card-foreground`,
    `--color-status-neutral-series`, `--color-status-info-series`.
- **`checkSdkUiCopies(srcDir)` in `@oxy-hq/sdk/testing`.** The no-copy guard for an app's
  tests: it reports every component in `src/` named like an `@oxy-hq/sdk/ui` piece, with its
  `file:line`, the SDK import to use and the allow-list line
  (`// oxy-ui-local: <Name> -- <why>`; a line without a reason is a finding too). Its names
  (`SDK_UI_PIECES`) are the ui entry's own components.

### Changed

- **Visible for every app that passes a `Stat` a `tip`: the desk shows its ⓘ by default**
  (decision 89). The focusable mark sits 8px after the figure, named "About <label>", and
  focusing it opens the tip, so a keyboard reaches every source; before, the tip was a hover over
  the tile only. `tipMark={false}` keeps the hover alone. A tile without a tip, and the phone's
  ⓘ after the label, are unchanged.
- **Visible for every app with a `TrendChart`: its labels hold 12px on screen at every width**
  (decision 92, the type scale's xs step). They were xs in viewBox units, so they grew with the
  drawn width (about 14.9px in a 694px dialog). The chart now measures its drawn width (a
  `ResizeObserver`; with no layout, a server render or jsdom, it takes its viewBox's size), and
  thins the month labels by it, so a wide chart may label different months. The 560 x 178
  geometry holds at 560px and wider; a desk chart drawn narrower now draws 1:1 (its viewBox the
  drawn width, 178px tall), as `fluid` does, since the scaled 560 left no room there for a 12px
  "Comparable" or y label.
- **The failed-read message is 13px at either size** (decision 85). `LoadError`'s default
  message was 12px and only `size="lg"` drew 13px: every app on the default size sees its
  failed-read callouts a step larger; drop any override of the message's size.
- **A `Chip` with neither `tone` nor `color` is `neutral`** (the muted tint and dim ink) where it
  was the bare tag. A chip given a `color` keeps no tone.
- **Tab counts print grouped**: `12,345`, in the strip and in the select it becomes.
- **A focus trap skips what a closed `<details>` hides**, its summary kept, so Tab wraps from the
  real last control.
- **The inset ring's forced-colours outline is drawn inside** (inset ring, segments, top-bar
  tabs), where a clipping container no longer cuts it; the outside ring's is unchanged.
- **`Field` wires only a labelable control** (`input`, `select`, `textarea`, `button`, `meter`,
  `output`, `progress`); a wrapper around one is left as it is.
- **A `ConfirmDialog` body's own children lose their margins**, so its gap is the only space
  between them in an app with no reset.
- A segment that says `aria-disabled` no longer changes on hover, as a disabled button already
  did (decision 66).

## [2.30.0] - 2026-11-20

### Added

- **`@oxy-hq/sdk/ui` and `@oxy-hq/sdk/ui.css`: the desk UI every app used to copy.** The
  reference app's gallery, built once: its 25 components (Button, DefaultTabs, ParentTabs,
  Stat, Chip, StatusDot, AlertStrip, AppMark, PanelTable, DataSheet, SortHeader, MetricCell,
  Sparkline, TrendChart, Panel, Modal, SidePanel, HoverHint, Field, SelectWithOther,
  AttachmentsField, ToggleChip, StringListEditor, Loading, LoadError), its 7 patterns as
  layouts with slots (ReorderSheet, MapSidebar, MapDetailCard, FlagRow, ReviewList,
  CoverageRebalanceList, FacilityActivityFeed), its `ui.ts` recipes as the `ui` object of
  `.oxy-ui-*` classes, and the pieces three or more apps carried that the gallery lacked
  (Segmented, TableScroll, LinkList, ConfirmDialog, ConfirmInline, CountBadge, AsOf,
  EmptyState, ErrorBoundary, SearchField, CopyButton, `downloadCsv`, Subnav, TopBarLeft, a
  router and URL state, `useRetryFocus`, `useReread`, scroll focus). Each piece starts from the
  best copy in the apps, not always the reference's: the side panel is the modal `<dialog>`
  over the whole window (decision 40b), Try again never drops focus to the page, tabs give way
  to a select when they do not fit, and TrendChart's month labels never collide. Plain CSS in
  `@layer oxy-ui`, reading only `theme.css` tokens; no Tailwind and no new dependency. Nothing
  changes for an app until it imports the entry.
- **`@oxy-hq/sdk/tailwind.css`.** Tailwind 4 `@theme` names (`--color-*`, `--text-*`, the
  radius steps, `--font-sans`, `--font-mono`) mapped onto `theme.css`, with the layer order
  that puts the SDK under Tailwind's utilities. Import it before `@import "tailwindcss"`.
- **`@oxy-hq/sdk/format`.** Money, percent, number, date, relative-time, plural and list words
  and CSV, one rule each where the apps' copies disagreed. Runs in a Function as well as a
  browser. With no `timeZone`, an instant shows in the viewer's zone in a browser and in UTC
  in a Function; calendar dates are never shifted. `formatAge` moves here (unchanged).
- **`checkThemeCss()` in `@oxy-hq/sdk/testing`.** One guard for an app's tests: it fails on a
  rule that restates an SDK token, a literal colour in `@theme`, a bare `.dark` / `.light`, or
  the app's own `color-scheme` or mode block.
- **`useKeptState` and `PhoneOptionSheet` in `@oxy-hq/sdk/shell`.** State kept across the
  phone and desk swap, and a full-screen choose-one sheet for the phone kit.
- **Theme tokens:** the type scale, desk sizes (control and table padding, measures), the
  radius steps, `--app-mark-ground` / `--app-mark-ink`, and `--status-info-series`.
- **44px controls and 16px fields under a coarse pointer** for every ui piece, honouring
  `data-oxy-touch-targets="off"`.

### Changed

- **Status colours pass contrast on a selected row.** Light `--status-success-series`
  `#159946` and `--status-warning-series` `#b27a04` (was 2.69:1 on `--row-selected`, now
  3.02:1); light `--chart-cat-5` follows the success series. Dark `--status-info` is its own
  token, `#93a2ff` (the info chip's text: 3.99:1 to 4.52:1), so `--link` keeps `#8696ff`. The
  chart's info line reads the new `--status-info-series` (dark `#4f67ff`), not `--brand`.

## [2.29.1] - 2026-11-18

### Changed

- **The phone nav sheet's appearance control is one footer row.** With `theme.css`, the
  Light / Dark / System control at the bottom of the nav sheet is now "Appearance" in the
  muted ink on the left and a compact switch of three icon-only buttons (sun, moon, monitor)
  on the right, on a quiet `--accent` track with no outer edge, under a `--border-subtle`
  hairline. The selected button takes the secondary button's fill (`--btn-surface`) and full
  ink; the others are muted ink, `--btn-surface-hover` on hover. Each button is 44px wide and
  40px tall inside a 44px tap area. The group is labelled by "Appearance"; each button has an
  `aria-label` and `aria-pressed` (it was a `radiogroup` of `radio`s with visible words).
  The modes, the stored choice, the sync with the person's Oxygen setting and the rail's
  switch are unchanged. Without `theme.css` there is no switch, as before.

## [2.29.0] - 2026-11-12

### Added

- **One light/dark mode per person, shared with Oxygen.** With `theme.css` imported,
  `OxyShell` applies the signed-in person's mode from the shell context's new `mode`
  field (`null` until they choose: light), keeps this origin's copy for the pre-paint
  (`OXY_THEME_PREPAINT`, an inline script anywhere in `<head>`, now applies it before first
  paint; it settles theme-color, a later pin and `__OXY_APP__` just before the first frame,
  and takes the mode back off if `theme.css` never applied), re-reads it when the tab is
  shown again, and follows the device under `system`. A Light / Dark / System switch at
  the bottom of the rail (and of the nav sheet on a phone) writes it back, so Oxygen and
  the person's other apps follow. `<html data-oxy-mode-pin="light">`,
  `<OxyShell modePin>` and `useOxyModePin` pin a mode and hide the switch; while a pin is in
  force the shell leaves this origin's copy alone, so a pinned shared screen keeps nobody's
  mode for the next load. Without
  `theme.css`, on a server without the field, and for a crew sign-in, none of it acts.
  New in `@oxy-hq/sdk/theme`: `OxyModeChoice`, `OXY_MODE_PIN_ATTRIBUTE`,
  `OXY_PERSON_MODE_KEY`, `applyOxyMode`, `isOxyThemeApplied`, `isOxyModeChoice`,
  `resolveOxyMode`, `readOxyModePin`, `readOxyPersonMode`, `rememberOxyPersonMode`,
  `readOxyPreviewMode`.
- **44px tap areas on the shell's own chrome on a touch screen.** With `theme.css`, under
  `pointer: coarse`, the rail's entries, the Today clock, the Ask button and the update pill
  answer a tap within 44px (an invisible `::before`; nothing drawn grows; in the phone layout's
  top bar too, where the bar's own centred box no longer pulls the area off the clock), the rail's entries
  sit 44px apart so each owns its area, and the mode menu's rows are 44px. `<OxyShell touchTargets={false}>`, `useOxyTouchTargets(false)` and
  `<html data-oxy-touch-targets="off">` turn them off for an enrolled store tablet. Without the
  theme or on a fine pointer, nothing changes.
- **Staging and sandbox previews never change the person's real mode.** On a staging or
  sandbox host the switch keeps a choice in that origin's copy and never sends it (the server
  refuses the write there too); it does not snap back on a re-read or a reload.
- **`<meta name="theme-color">` follows the mode.** With `theme.css`, the shell and
  `OXY_THEME_PREPAINT` set every theme-color meta (adding one where there is none) to
  `--background` in the mode in force, pins included, and follow any later change of
  `.dark` or `data-oxy-mode` on `<html>`. New: `syncOxyThemeColor`.
- **The reference's names an app copied in by hand.** `--status-info`, `--status-info-bg`,
  `--status-info-line`, `--shadow-elevated`, `--select-chevron` and `--dot-*-glow`, in both
  modes, at the reference's values (light glows follow the corrected light dots).
- **`--chart-1` … `--chart-5` are the reference's sequential blue ramp** (`--chart-seq-1…5`),
  as its own are; categorical series take `--chart-cat-*`.
- **The breadcrumb's home link honours `data-oxy-touch-targets="off"`**, at its drawn size on
  a store tablet as the other touch rules are.
- **A `PhoneRow` in a `PhoneSheet` keeps its whole height** with the theme, under a
  `display: contents` wrapper too.
- **`PhoneKpi.tone` takes `warn`.** With the theme it is the warning status ink
  (`--status-warning`), as the desk draws it; without the prop the delta stays neutral.
- **Overlays blur the page behind them.** With `theme.css`, a design-system rule beyond the
  reference (whose dialogs only dim): a new token, `--scrim-blur` (8px), and two rules in
  `@layer theme`. Every native modal `<dialog>`'s `::backdrop` blurs by it and keeps the
  app's own scrim colour; the class `oxy-scrim` gives any other scrim (a fixed div, a Radix
  overlay) `--scrim` and the blur. The shell's nav drawer (the phone menu) dims by `--scrim`
  and blurs too. Dark `--scrim` is black at 0.5 (light keeps the reference's 0.35), so the
  blurred page still reads through it. Under `prefers-reduced-transparency: reduce` the blur
  is 0px and the scrim stays. Without `theme.css`, nothing changes: the drawer keeps 2.28's
  black at 0.8 and no blur.
- **The top bar's Ask Oxygen is the reference's secondary button**, like the app buttons
  beside it (`btnCompactSecondary`): with the theme it is filled `--btn-surface` on its
  `--border-subtle` edge in full ink, hover and open move the fill to `--btn-surface-hover`,
  and the ⌘K hint is a `--muted` chip in the muted ink. Its size, icon, hint and 44px touch
  reach are unchanged; without the theme it is 2.28's outlined button.

- **`@oxy-hq/sdk/theme.css`: the reference app's design, applied by importing it.** The
  design standard for every custom app is the oxygen reference app
  (`oxy-hq/reference-app`). `theme.css` carries its values as the theme contract's
  custom properties, each primitive citing its source line: surfaces and ink tiers,
  the frame line apart from the 3:1 control line, one brand blue, the status family,
  square corners, the system face first, 32px desk and 44px touch controls, the 1.5px
  focus ring, one elevation. Light on `<html>`, dark under `.dark` or
  `data-oxy-mode="dark"`. Every selector sits in `:where()`, so an app's own token
  declarations still win; adopting the theme means importing it and deleting them.
  The old names it maps onto the theme (`--radius`, `--font-inter`, `--ring`,
  `--color-border`, `--chart-1…5`, …) sit in `@layer theme`, so an app's Tailwind
  `@theme` keeps them too.
  Three reference values that miss a contrast obligation are corrected in the theme,
  with a comment each (dark `--brand`, dark `--disabled-foreground`, light
  `--status-warning-series`).
- **The shell and the phone kit read the theme.** Corners from `--radius-control`,
  `--radius-surface`, `--radius-overlay`, `--radius-pill`, `--button-radius`,
  `--card-radius`, `--input-radius` and `--badge-radius`; the rail, top bar, tab bar,
  sheets, cards and KPI tiles edged with `--rail-border`, `--topbar-border` and
  `--card-border`; the Ask button and the kit's chips, segments and secondary buttons
  with `--button-secondary-border` (fields keep `--border`); the rail and bars on `--rail-bg` and
  `--topbar-bg`; touch targets from `--control-h-lg`. Each is read as
  `var(--theme-name, <what 2.28 drew>)`, so an app that does not import the theme
  renders as before, its own `--radius`, `--border` and `--oxy-shell-*` overrides
  included.
- **With the theme, the shell's and the kit's own parts follow the design system.**
  Every focus ring is the reference's one ring, 1.5px of `--focus-ring-color` at full
  strength (2.28 drew 2–3px of the host ring at 45–60%, measured 2.82:1 on the rail and
  2.90:1 on the top bar), and a focused rail tile is drawn at full strength, because its
  0.6 resting opacity dimmed the ring with it. The kit's primary button is the brand fill
  (`--button-primary-bg` / `--button-primary-fg`), not the ink `--primary`; its
  secondary button is `--secondary`; its selected segment is `--segment-selected`, not
  a raised page-colour tile. `OxyChat`'s Ask is the primary pair (it was white on the
  dark link blue, 2.69:1) and its field and buttons take `--input-radius` and
  `--button-radius`. The ⌘K hint is 11px, the reference's smallest step. The Today
  clock keeps its whole 44px reach on a phone (its neighbour's reach took its right
  edge). The kit's segment track is `--segment-track`, so the selected segment steps off
  it as on the reference's desk strip (1.27:1 light; on the old track it was 1.00:1). The
  phone tab bar's current tab, icon and label, stays the accent blue 2.28 drew, now the
  theme's `--link` (7.77:1 on the bar in light, 7.09:1 in dark; the floors are 4.5:1 for
  the 11px label and 3:1 for the icon), with its label at 600: on a phone the bottom
  tab bar is the one selection drawn in blue, while the desk's navigation, tabs and
  segments keep a monochrome selection. In the top bar the breadcrumb's "/" is the muted
  ink at full strength (6.85:1 light, 8.25:1 dark; at 40% it was 1.88:1 and 2.29:1,
  every desk screen's only text under 4.5:1), the home link takes the one ring instead of
  the browser's outline, and the Today clock is drawn in the shell's sans face with
  tabular figures, not monospace. `--secondary`, `--segment-track`,
  `--segment-selected`, the current tab's `--link`, the "/" and the 11px are switched on
  `--oxy-theme-contract` rather than read as plain
  names, because apps set those names for themselves; the clock's face and the home
  link's ring switch on it through a style query, which no colour mix can carry.
- **With the theme, the kit's tones are the design system's status family**, the one the
  desk draws, so one status no longer reads two ways. The kit mixed its own from the
  destructive and success tokens: a warning pill was `#573e00` on `#dbd4c0` beside the
  desk chip's `#865108` on `#fbf0ce`, an error pill `#782420` on `#e3cbcc`, and the dark
  row dots `rgb(218,161,0)`, `rgb(250,142,142)` and `rgb(170,170,179)`. Now a
  `PhoneCard` status pill is the reference's chip pair (`--status-*-bg` with
  `--status-*`; neutral `--status-neutral-bg` with `--dim-foreground`), a `PhoneKpiStrip`
  delta the status ink (neutral `--muted-foreground`), and a `PhoneRow` dot `--dot-*`
  (neutral `--status-neutral-series`). Measured on the kit's own surface, the pills read
  5.06:1 or more light and 6.53:1 dark, the deltas 5.63:1 and 8.05:1, the dots 3.42:1 and
  4.93:1; 56 contrast cases hold them, each on the ground the kit draws it on. Switched
  on `--oxy-theme-contract`, because Store Ops sets the status names for itself.
- **`PhoneTabItem.badgeTone`.** A tab badge is the shell's red, as before, unless it has a
  tone: then it is the kit's tone pill, and with the theme the reference's tab count, the
  tone's status pair or, for `neutral` (a count that is just a count), `--muted` in the
  muted ink. More's dot takes the strongest tone behind it once every badge there has one.
- **Theme names for the reference's own parts:** `--segment-track`,
  `--segment-selected`, `--btn-surface`, `--btn-surface-hover`, and the status dots
  `--dot-success`, `--dot-info`, `--dot-warning`, `--dot-error`, light and dark, from
  the reference's values. Light `--dot-success`, `--dot-warning` and `--dot-error` are
  corrected with a comment: on a selected row over the light canvas the reference's
  `#22c55e`, `#f5b000` and `#ef4444` measure 1.71:1, 1.42:1 and 2.85:1, under the 3:1 a
  status mark needs. Each takes the colour closest to the reference's at the same hue that
  clears 3:1 on every light ground a dot sits on (the canvas, the card, `--muted`, a
  selected row on either, the segment track and selected segment, the shell rail at rest
  and selected, the top bar, white): `#059341`, `#a5770d` and `#eb3c3e`, 3.02:1 or more on
  the worst of them. Light info and all four dark dots keep the reference's values, which
  clear 3:1 on every ground of their mode (dark info 3.44:1 at worst). 32 more contrast
  obligations hold them, each dot on each of those grounds (40 dot pairs in all).

### Fixed

- **A focused shell or kit control shows focus under forced colours.** Each draws a
  transparent 2px outline beside its ring: invisible otherwise, it is what Windows
  contrast themes paint, since they drop the box-shadow rings.
- **No ⌘K hint on a phone on its side.** At 844x390 the Ask button is wider than the
  640px breakpoint, so it showed the keyboard hint, at 10px, to a person with no
  keyboard. The hint is hidden whenever the phone layout is on.
- **A corner computed from the radius never goes negative.** Every
  `calc(var(--oxy-shell-radius) - Npx)` is clamped at 0, so a zero radius gives a
  square corner by value rather than by the browser dropping an invalid length.
- **No `PhoneSegmented` label is cut while there is room.** Segments are equal while every
  label fits an equal share, as before; a label that does not keeps its whole width and the
  others share the rest (at 320px "Attention (12)" was cut beside options with room to
  spare). A row still too wide for the screen scrolls sideways, without a bar, faded on the
  edge with more options beyond it, and keeps the selected option wholly in view, clear of
  the fade. A row that fits lays out as it did, with or without the theme.
- **A `PhoneCard` figure's value is never cut through a glyph.** Its parts wrap onto a second
  line rather than run past the column's edge ("3.73" and "vs 4.14"), text alone that still
  does not fit ends in an ellipsis, and an element of the app's that holds only text wraps
  inside itself; the whole value stays in the DOM, so a screen reader reads all of it. This covers an app's own `oxy-phone-card__figures` list too. A value that
  fits lays out as it did. With the theme, a card has one figure column per figure (up to
  three), so two figures share its width rather than wrapping beside an empty third column.
- **With the theme, a `PhoneSection`'s title lines up with the page title and its cards.**
  2.28's 4px inset (the reference phone's too) set it 4px right of both, and its action 4px
  in from their right edge. Without the theme it keeps the 4px.
- **With the theme, a `PhoneSheet`'s Back shows its focus ring only when the sheet was opened
  from the keyboard** (the control that opened it showed focus, as the browser judges it).
  Back still takes focus on open; a tap, a click or a sheet open on arrival no longer rings
  it, which Chrome's guess for a script's focus did. Without the theme, focus is as 2.28 set it.

## [2.28.1] - 2026-11-11

### Fixed

- **Closing a PhoneSheet puts the page back exactly where it was.** The scroll lock
  froze the page behind but did not remember its position, so a list that rendered
  shorter while a sheet was open (or in the commit that opened it) came back at the
  top, and a Back could land on the browser's saved, clamped position. The lock now
  keeps the position the person left, puts it back when the last sheet (or the Ask
  panel on a phone) closes, and holds it for a second while the list is still too
  short or a Back lands. It lets go the moment the person touches the page, anything
  else scrolls it (an app bringing a saved row into view), the URL is another page's,
  or `PhonePage`'s `scrollKey` starts a new tab at its top.
- **A phone on its side gets the navigation drawer instead of a 32px rail.** At
  844x390 the rail's items were 32px, and at 44px a workspace's apps do not fit the
  height. Under `NAV_SHEET_QUERY` (below 768px, or a coarse pointer no taller than
  500px) the shell shows the menu button and its 44px drawer rows; a rail still
  shown there (`hideTopBar`, an app's own `ShellRail`) has 44px items. A tablet and a
  desktop keep the rail as it was.
- **A read that answers 200 without rows no longer takes the app down.** `useQuery`
  cached the body before reading rows from it, so the next component mounting the
  same query threw `rows.map` of undefined inside the SDK's effect and React
  unmounted the app. Such a body is now an `OxyApiError` with code `malformed_read`
  in the hook's `error`, never cached, and `refetch` recovers; `useSemanticQuery`
  reports the same error instead of a TypeError. A body cut off mid-read keeps its
  own (network or abort) error.

## [2.28.0] - 2026-11-10

### Added

- **Ask an agent and get the answer in one call.** `useAskAgent()` (and `askAgent`,
  for code holding a fetcher) starts an ask and waits for it, up to 45 s
  (`waitSeconds`), returning its `status` and the `answer`, the `error`, or the
  agent's clarifying `question` and `suggestions`. An ask still going when the wait
  runs out comes back `status: "running"` with its `runId` and `threadId`: it keeps
  running, and `useAgentRun`'s stream or a later ask on the same thread picks it up.
  For a chat UI keep `useAgentRun`; this is for a button that fills in a sentence or
  a batch over many rows. Needs a server with `wait_seconds` on asks (#4040); an
  older one answers every ask as `running`.

## [2.27.3] - 2026-11-10

### Fixed

- **The phone's Back closes the top bar's Today panel.** The panel held no history
  entry, so Back with it open left the app. In the phone layout (`PHONE_LAYOUT_QUERY`)
  an open panel now holds one in the same coordinator as `PhoneSheet` and the drawer:
  Back closes it and the app stays where it was. A row or the "Open Oxygen home" link
  replaces that entry, as a drawer row does, so Back from the next page returns to the
  app. A tablet (a store's kiosk among them) and a desktop hold no entry, as before;
  `<OxyShell plainClock>` has no panel at all.
- **Ask Oxygen covers a phone's screen instead of squeezing the app.** Docked beside
  the app as on a desktop, the panel took `min(23rem, 85vw)` of a 390px screen and
  left the app a ~60px column, its text wrapping a letter a line and the tab bar's
  labels cut. In the phone layout (`PHONE_LAYOUT_QUERY`, upright or on its side) the
  open panel is now a fixed full-screen layer: the app keeps its width behind it and
  is `inert` until it closes. Back closes it (one history entry, as Today holds), as
  does its close button, now 44px like its other header buttons. It follows the visual
  viewport, so the composer (16px) sits above the keyboard. "Open chat in Oxygen"
  replaces the entry rather than leaving it behind. A tablet and a desktop keep the
  docked side panel exactly as it was.

### Added

- **`<PhoneTabBar historyEntry>`: the phone's Back returns to the first section.**
  Opt-in, off by default. `PhoneTabBar` taps push nothing, so in an app that keeps its
  section in state (or replaces the URL with it) Back after switching sections leaves
  the app. With `historyEntry` on, Back works as a native app's bottom navigation
  does: from any section but the first it calls `onSelect` with the first item's id,
  and from the first it leaves. The bar holds one entry while away from the first
  section, however many tabs are tapped, so Back never walks through every tap. Picks
  from More count the same, Back with More open closes More first, and an app opening
  on another section gets the entry too. Only in the phone layout. **Leave it off when
  `onSelect` pushes history** (a route or `?tab=` per section): that router already
  steps Back between sections, and with both, Back swaps between two sections forever
  and never leaves the app. That is why it is not the default.

## [2.27.2] - 2026-11-09

### Fixed

- **The phone kit's controls are at least 44px to tap.** Measured on phones in
  2.27.1: `PhoneFilterChip` was 36px tall, `PhoneSegmented`'s segments 40px, and a
  clickable `PhoneRow`'s button 39px inside its 56px row. The chip is now 44px, the
  segments 44px in a 48px track, and the row's button is the row's full height and
  runs from its left edge (the row is as tall as before and nothing in it moves). A
  clickable `PhoneCard`'s button takes in the card's padding above and below its text,
  so a one-line card's is 44px, not 20; the card looks the same. `PhoneHint` keeps its
  24px ⓘ and its 44px reach, which no longer loses 2px to a row right under it.
- **The top bar's controls answer a tap within 44px in the phone layout.** The menu
  button (36px), the Today clock (28x24), Ask, the update pill, the drawer's close
  (32px), and a button or link an app puts directly in the bar with `topBarExtra` (an
  account menu, 38x32) each get an invisible hit area at least 44px each way, centred
  on it. Nothing drawn grows; upright, the Today icon gets 2px either side so a 32px
  neighbour's reach does not take from its own. Only under `PHONE_LAYOUT_QUERY`: a
  landscape touch tablet, such as a store's kiosk, and a desktop keep the bar exactly
  as it was. On an app's own control the reach is its `::before`, in the base layer:
  the app keeps its own `position`, and in the phone layout the shell owns that
  `::before`, so a badge on such a control belongs in a child element.

## [2.27.1] - 2026-11-06

### Added

- **`<OxyShell plainClock>` keeps the top-bar clock a plain time.** No Today
  button, no Today panel, no `GET /api/home/widgets` read and no clock icon at phone
  width: the clock is the 2.23 `WorkspaceClock`. For a screen that is not one
  person's, such as a store's shared tablet. The default is unchanged, the clock
  opens Today.

### Fixed

- **A phone sheet's Back no longer leaves the app under the platform's runtime.**
  The runtime turns a push made before the person has touched the page into a
  replace, so a `PhoneSheet` or drawer open at load (a pasted link, a reload) had no
  history entry under it, and its Back and the phone's Back landed on the page before
  the app. The coordinator now pushes through the runtime's own opt-out, for its push
  only, so an app's boot-time pushes are still made replaces and the app adds nothing.
  It also stops assuming a push landed: where one did not (another rewriter, an older
  host), the sheet closes without a step off the page, and the entry is made on the
  first activation. Picking a drawer row with no entry under it navigates rather than
  replacing the page.
- **A reload with a sheet open reuses that sheet's entry.** It used to stack a new
  one each time, each with a URL that opened the sheet again on the way back down.
  Marks written by 2.27.0 are not reused, since they may sit on an entry that was
  never pushed.

## [2.27.0] - 2026-11-04

### Added

- **Phone kit in `@oxy-hq/sdk/shell`: the pieces a custom app's phone view is
  made of.** `usePhoneLayout()` (with `PHONE_LAYOUT_QUERY`) is `true` on a phone
  upright or on its side, so an app renders
  `phone ? <OrdersPhone /> : <OrdersDesktop />` from the same data hooks and
  leaves its desktop layout as it is. The view is built from `PhonePage` (a
  frame that scrolls with the shell, with 16px safe-area gutters and a bottom
  dock), `PhoneTabBar` (up to five slots, past five a More sheet),
  `PhoneSheet` (full screen, Back and Escape, focus in and back, the page behind
  locked, the phone's Back closes only the top sheet), `PhoneStickyAction` and `PhoneButton` (48px), `PhoneKpiStrip`,
  `PhoneSection`, `PhoneRow`, `PhoneCard` and `PhoneCardList` (a table row as a
  card with up to three figures; two columns on a phone on its side),
  `PhoneSegmented`, `PhoneFilterChip` and `PhoneHint` (an ⓘ that explains on a
  tap, replacing hover-only tooltips). Styles ship in `shell.css` under
  `oxy-phone-*` and use only the shell's tokens, so they follow the host theme
  and a `.dark` ancestor, with corners from the app's `--radius` (at `0`, every
  control is square; only status dots stay round), text in the app's `--font-sans`
  when it has one, and Phosphor glyphs drawn inline, so there is no new dependency.
  Inside a phone page text stays at 11px or more, actions at 44px or more, and
  small fields are raised to 16px. An app that renders no phone view gets only
  the shell fixes below.

### Fixed

- **The shell works better on a phone.** On a touch phone, upright or on its
  side, small text fields inside `OxyShell` are raised to 16px, so iOS Safari no
  longer zooms the page when someone taps a filter or a search box (it zooms on
  anything smaller and does not zoom back). The rule only raises: a field set
  larger anywhere keeps its size. It reaches Tailwind's `text-3xs`, `text-2xs`,
  `text-xs` and `text-sm`, and any field no class sizes. **Not covered, and still
  under 16px:** an arbitrary small size (`text-[11px]`), another `--text-*` token
  of the app's own, and an unlayered `font-size` rule under 16px; nor a dialog an
  app portals to `<body>`, which is outside the shell. On any touch screen the
  workspace link in the top bar's breadcrumb, the way home from every app, fills
  the bar's height instead of its 20px text box. The Today popover's height on a
  phone follows the visible screen (`100dvh`), so Safari's toolbar no longer
  covers its last rows. A mouse-driven desktop and a tablet's field sizes are
  unchanged.

### Changed

- **The shell's top bar no longer shows "Sys: Connected".** `SystemIndicator`
  renders nothing while the browser is online and a red "Offline" when it is
  not. It only ever mirrored `navigator.onLine`, so the pulsing green dot was a
  light that could not go out. The pulse animation and the mono "Sys:" label
  are gone; an app that placed `SystemIndicator` itself needs no change.

## [2.26.0] - 2026-11-02

### Added

- **preact 11 for `@oxy-hq/sdk/email`.** The `preact` peer range is now
  `^10.0.0 || ^11.0.0`, so an app can move its email templates to preact 11
  without a peer warning; the `functions` template from create-oxy-app 0.10.0
  starts on it. `render(Component, props)` is unchanged and the HTML is the
  same on both majors, with one exception that comes from preact itself:
  **preact 11 ignores `defaultProps` on a function component**, without an
  error (it moved to `preact/compat`), so a template that set
  `Greeting.defaultProps = { name: "friend" }` renders the prop as missing.
  Give the default as a default parameter (`function Greeting({ name =
  "friend" })`), which works on both. Numeric styles still render with `px`
  (`padding: 24` → `padding:24px`): preact 11 dropped that from its DOM
  renderer, but email HTML is serialized by preact-render-to-string, which
  keeps it. A new test bundles the renderer as `oxyc publish` does and runs
  it in a bare V8 context with no Node globals, so a later preact release
  that needs a timer or `TextEncoder` fails CI instead of failing in the
  Functions isolate.

## [2.25.0] - 2026-11-01

### Added

- **Today panel from the top-bar clock in `<OxyShell>`.** The clock is now a
  button. It opens a "Today" panel listing the viewer's home widget cards
  from every app they can open (`GET /api/home/widgets?org=<slug>`), so an
  item that needs them is one click away from inside any custom app. Rows
  that need a look come first (level `attention`/`critical`, or a card with
  buttons), then calm ones, then cards that failed ("Couldn't load"); each
  row shows the app icon (or initials), title, headline and a small ring
  when the view reports progress, and is a plain link (same tab) to the
  view's link, else the app. The footer opens Oxygen home. Nothing is
  requested until the panel is first opened; a failed request shows
  "Couldn't load Today." and never reaches the host app. Escape, a press
  outside and tabbing away close it; Escape returns focus to the clock. The
  panel arrives and leaves like Oxygen's overlays and fades only under
  reduced motion; its styles (`oxy-today-*`) ship in `shell.css`, so
  upgrading is enough. On a phone the clock shows as an icon and the panel
  spans the width. No new exports; `WorkspaceClock` is unchanged, and the
  button appears only in the wired `OxyShell` once its org is known.

## [2.24.0] - 2026-10-31

### Added

- **`@oxy-hq/sdk/motion.css`: Oxygen's motion language, for custom apps.**
  The same file Oxygen's web-app loads: duration tokens
  (`--oxy-duration-instant/fast/base/slow/focal`, 80/120/160/240/600 ms),
  curves (`--oxy-ease-out/in/in-out`), and app-wide rules keyed on the
  `data-slot` and `data-state` attributes shadcn/Radix render: dialogs,
  sheets, menus, popovers, selects and tooltips arrive in 160–240 ms on an
  expo curve and leave faster, settling out of their trigger (scale 0.97, a
  4px nudge) rather than popping. A shadcn button gives under a press, through
  a rule in `@layer components`, so an app's own transition and scale classes
  on a button still win; to ease the press, include `scale` in the button's
  transition list.
  Reduced motion keeps the fades and drops all travel. `shell.css` imports it,
  so an app on `<OxyShell>` gets it by upgrading. A Tailwind app maps the
  tokens in its `@theme` (the create-oxy-app templates show how) so its own
  `transition` and `ease-*` utilities move like Oxygen too.

### Changed

- **`<OxyShell>` moves on the shared tokens.** The rail, top bar, Ask dock and
  phone drawer took their timing from ad-hoc values (0.15s, 0.2s, 0.25s,
  `ease`); they now use `motion.css`. The rail tooltip nudges 4px instead of
  8px. Under reduced motion the phone drawer fades instead of not animating at
  all, the tooltip fades without travel, and the live indicator stops pulsing.

### Fixed

- **`useDockPrefetch` is exported from `@oxy-hq/sdk/shell`.** 2.22.0 named it
  as the hook behind the dock's prefetch but did not export it, so an app that
  builds its own chrome from `ShellRail` could not use it. The dock now
  prefetches only where the opened app can tell it came from the prefetch
  (`deliveryType` on its navigation entry), so its open is still counted.
  
## [2.23.1] - 2026-10-30

### Fixed

- **`<OxyShell>`'s rail tooltip is readable in a dark app without a `.dark`
  class.** The tooltip's text is the host's `--foreground`, but in the light
  scope its box was the fixed `--zinc-200` (#e4e4e7). A dark-only app that
  never sets `.dark` got near-white text on light grey (1.08:1, measured on
  two production apps). The box is now the host's `--muted` in both scopes,
  falling back to `--zinc-200` and then #e4e4e7. The web-app's light `--muted`
  is `--zinc-200`, so the product does not change. An app gets this by
  upgrading and republishing.

## [2.23.0] - 2026-10-28

### Changed

- **`<OxyShell>` on a phone: a menu instead of the icon rail.** Below 768px
  the 48px rail is hidden and a menu button at the start of the top bar opens
  a left drawer with the same entries — HQ, Chat, every app — each with its
  name, as 44px rows at full opacity, the current app marked. The rail's names
  were hover tooltips, which a touch screen never shows. A pick, the overlay or
  Escape closes it, and focus returns to the menu button; while open, Tab stays
  inside it, and widening past 768px closes it. It renders after the app's
  content, so an app's own `fixed` bar at z-index 50 sits under it. With `hideTopBar`
  there is nowhere for the button, so the rail stays at every width. An app
  gets this by upgrading the SDK and republishing; nothing to change in its
  code.

### Added

- **`NavSheet`** (from `@oxy-hq/sdk/shell`) — that drawer as a presentational
  component, taking `ShellRail`'s `groups` / `footerItems` / `bottom` plus a
  `title`, for a host composing the shell pieces itself.

## [2.22.0] - 2026-10-28

### Added

- **The dock warms the app you are about to open.** `<OxyShell>` installs a
  Speculation Rules prefetch for the rail's other apps on hover (Chrome and
  Edge), so switching apps from inside one skips the page's round trip. A
  prefetch, never a prerender: nothing runs for a hover. `useDockPrefetch` is
  the hook behind it.

### Changed

- **`<NotificationsBell>` opens another app's notification, or a platform
  notice, in this tab**, as Oxygen's home page and dock now do, instead of a
  named tab per app (`oxy-app-<org>-<app>`). An absolute URL still opens in a
  new tab. Marking an item read is a `keepalive` request, so leaving the page
  does not cancel it.

### Removed

- **Source for Data Apps (`.app.yml`), which the server no longer serves.**
  Oxygen removed Data Apps (every `/apps/*` route now answers 404), and this
  deletes the client code that called them: `OxyClient` (`listApps`,
  `getAppData`, `runApp`, `getDisplays`, `getFile`, `getFileUrl`,
  `getTableData`), `OxySDK` (`loadFile`, `loadFiles`, `loadAppData`),
  `OxyProvider`, the Parquet reader, the Data App types (`AppItem`,
  `AppDataResponse`, `GetDisplaysResponse`, `DataContainer`, …) and the
  `react-vite-demo` example. **No published API changes:** none of this was
  reachable from any entry point since v2 (see 2.0.0 — the v1 `OxyClient` /
  `OxySDK` stack was dropped from the exports), so the built package is
  unchanged and this is not a major release.

## [2.21.0] - 2026-10-17

### Added

- **`oxy-app.json` has one definition, and it is this package's.** The
  `OxyAppManifest` type now carries every field the server honours —
  `rateLimitPerMinute`, `fetch.maxResponseBytes`, the per-function `storage`
  capability, `description`, `icon`, `art`, `status`, `build`, `environments`
  and `$schema` — each documented with what the platform does with it, and a
  JSON Schema is generated from it: `schema/oxy-app.schema.json`, shipped in
  the package (`https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json` for
  a manifest's `$schema`) and pinned by a test that regenerates it.
- **`@oxy-hq/sdk/manifest`** — a pure-TypeScript entry (no React, no DOM)
  with the manifest types, the schema (`OXY_APP_SCHEMA`), the two server name
  rules (`isValidSlug`, `isValidFunctionName`, their patterns and sentences),
  `validateOxyAppManifest(raw)` — a dependency-free validator over the schema
  plus the rules it cannot say, returning `{ level, path, message }` issues in
  one wording — and the notification catalogue's checks. `oxyc validate`,
  `oxyc publish`, `@oxy-hq/vite-plugin` and this SDK's own loader all run it;
  the two packages that carried byte copies of `notification-catalog.ts` now
  import it.

### Changed

- The loader (`loadCustomAppManifest`) validates against the schema instead of
  its own hand-written checks. The same manifests are refused, in the same
  sentences (`schemaVersion must be 2 …`, `` `slug` is required … ``,
  `function "x" must enable at least one of route/schedule/airwayStep/webhook`);
  a field the schema does not know is now logged as a warning rather than
  silently dropped, and the resolved `manifest` keeps every field the file
  holds.

## [2.20.0] - 2026-10-17

### Added

- **`ctx.platform.facts()` tells a customer's own use from ours, and prices
  each account's LLM usage.** For an internal app (the accounts view staff
  read); all four fields are optional, since a server older than this release
  sends none of them.
  - Per app, `customerLastActiveAt`: the latest production view by a non-staff
    person in the last 180 days. `lastActiveAt` is unchanged and still counts
    staff's opens, so a customer's app we keep opening no longer looks used.
  - Per app, `people7d`: distinct non-staff people with a production view in
    the rolling seven days `last7d` covers.
  - Per window (`weekly[]` and `last7d`, `OxyPlatformWeek`), `llmCostUsd`: the
    dollar cost of the window's `agentRuns` and every run they delegated to,
    priced exactly as Admin → Metrics prices it. Platform runs and staff
    previews and dry runs are left out, as they are from `agentRuns`.
    `llmUnpricedRuns` counts the runs on a model with no price, which add 0.

## [2.19.0] - 2026-10-16

### Added

- **A notification catalogue in `oxy-app.json`.** The app-level
  `notifications` block labels, groups and ranks the kinds an app's functions
  send — the tree people mute in (a whole app, a category, one kind) and the
  one a phone groups by. `categories` are paths of up to 3 segments
  (`operations/food-safety`; a declared path's parents are implied); each kind
  in `kinds` has a `category` (default `general`), a `label`, a `level`
  (`passive` | `active` | `time_sensitive` | `critical`; a function's
  `"critical": true` still makes it critical) and up to 3 `actions`
  (`{ id, label ≤ 24, function?, destructive? }` — with `function`, a tap calls
  that function as the person, then marks it read). Which kinds a function may
  send is still its own `notify.kinds`. Types: `OxyAppNotificationsManifest`,
  `OxyAppNotificationCategory`, `OxyAppNotificationKind`,
  `OxyAppNotificationAction`, `NotificationLevel`, and
  `OxyAppManifest.notifications`.
- **`ctx.notify({ …, thread })`** — group notifications about one thing
  (`store:${id}`, ≤ 128 characters). Inboxes fold a run of one thread into its
  newest; only a hash is stored, and items carry it as `thread`. The test
  context refuses a blank, over-long or non-string `thread` with the server's
  `InvalidNotifyPayload` wording.
- **`NotificationItem` gains `category` (`{ path, labels }`), `kind_label`,
  `level`, `thread`, `actions` and `url`**, all optional — a server older than
  the catalogue sends none of them and everything renders as before.
  `NotificationsPage.next_before` is the cursor for the next page.
- **`useNotifications({ category, kind, thread })`** narrows the list, the
  polled count and "Mark all read". `category` covers its subtree and names
  this app, so it needs the app id oxy injects. On a server older than the
  catalogue the hook filters the page itself and marks the listed items one
  by one rather than send a `read-all` that server would not narrow.
- **`<NotificationsBell />` shows the catalogue.** Each row names its category
  under the title (nothing for `general`), marks `critical` and
  `time_sensitive` notifications with a leading edge — themeable with
  `--oxy-notifications-critical` and `--oxy-notifications-time-sensitive` — and
  folds a run of one thread into its newest with "+N more"; opening it reads
  the whole run. `collapseThreads(items)` is the same folding for an app that
  draws its own.
- **`@oxy-hq/sdk/testing`:** `createTestContext` refuses a manifest whose
  catalogue publish would refuse, listing every problem;
  `createTestInbox({ manifest })` gives received items the category, label,
  level and actions the catalogue gives them, hashes `thread` as the server
  does (`threadKey`), and answers the `source`, `category`, `kind`, `thread`
  and `before` filters. `checkNotifications(manifest)` and
  `resolveNotificationKind(manifest, kind)` are exported for an app's own
  tests.

`oxyc validate`, `oxyc publish` (before the build) and `@oxy-hq/vite-plugin`
(`pnpm dev` and `pnpm build`) check the catalogue with the same rules as the
server and list every problem at once.

## [2.18.0] - 2026-10-14

### Added

- **`ctx.notify` — tell people in this org something.** A function names
  people (`{ userId }`, `{ role, location? }`, `{ team }`), a declared `kind`,
  a `title`, and optionally a `body`, a `link` inside the app, `email`, `slack`
  and an `idempotencyKey`. Each person gets an inbox row (frontline crew
  included), then push, email or Slack where they can be reached. Needs
  `"notify": { "kinds": { … } }` on the function in `oxy-app.json` and an OLTP
  store for the org. Kinds starting with `oxy.` are Oxygen's own and refused
  (`NotifyKindReserved`). `NotifyInput`, `NotifyResult` and `NotifyTarget` are
  exported from the root, and `createTestContext` enforces the capability and
  the declared kinds in the host's words and records each send on
  `state.notifications`.
- **`useNotifications()` and `<NotificationsBell />` — the inbox inside the
  app.** A custom app opens in its own tab, so people who live in one app
  (crew on a kiosk, a single-app user) never saw Oxygen's bell. Drop
  `<NotificationsBell />` into the app's header: a bell with an unread badge
  that opens this app's notifications, newest first, with "Mark all read". A
  tap on one of this app's opens it in place — `onNavigate(link)` for the
  app's router, else the page navigates to the link under the app's base path;
  another app's or one of Oxygen's own opens where Oxygen would, in a new tab.
  It polls the unread count every 30 s while the tab is visible and fetches
  the list only when opened. `scope: "all"` shows the whole inbox. Inline
  styles only; theme it with `--oxy-notifications-*` CSS variables (it also
  follows `<OxyShell>`'s tokens and Tailwind v4 `--color-*` ones).
  `useNotifications()` is the same data for an app that draws its own. On a
  staging or `dev-*` host it is read-only (`readOnly: true`): read state is
  shared with production, so the bell shows no "Mark all read" and a tap does
  not mark read.

  **Renders nothing where there is no inbox:** a server without
  `/api/notifications`, a page nobody is signed in to, or `pnpm dev` (which
  has no app id; use `scope: "all"` there). It needs a server that takes
  `app_id` on the inbox routes; on one that ignores it, the app's inbox hides
  itself rather than show, or mark read, other apps' notifications.
- **`createTestInbox()` in `@oxy-hq/sdk/testing`** — a `fetch` that answers the
  inbox routes from items a test holds, for `<OxyAppProvider fetcher={…}>`.
  `inbox.receive(state.notifications[0].input)` shows what a function test
  sent with `ctx.notify`.

## [2.17.0] - 2026-10-03

### Changed

- **`useQuery` sends its `params` to the server instead of writing them into
  the SQL.** The hook used to substitute `{{ params.X | sqlquote }}` itself,
  doubling `'` and nothing else. That is DuckDB's and Postgres's whole rule and
  nobody else's: ClickHouse, MySQL, Snowflake and Redshift also read a
  backslash as an escape, and BigQuery reads a backslash and does not read `''`
  as a quote. So a value holding a backslash — `C:\`, or `x\'` — broke the
  query on those engines, or changed what it asked. The browser cannot know
  which engine a project's database is; the server can. The request now carries
  the template untouched plus `params: { … }` for the names it uses, and the
  server writes each string by the rule of the engine that will read it.
  `{{ params.X }}` with no filter is still written unquoted, a number or
  boolean as itself, and a missing or `null` param as `NULL` (a `NaN` or
  infinite number is now `NULL` too, where it used to be written as the word).

  **Needs a server that accepts `params` on `POST /api/projects/{id}/query`.**
  An older one answers `400 invalid request body: unknown field "params"` to a
  query that uses a placeholder; a query with no placeholder sends the same
  request it always did. Not a security boundary before or after: that endpoint
  runs any read-only SQL a signed-in member sends.

## [2.16.0] - 2026-09-23

### Added

- **`@oxy-hq/sdk/testing` — a typed test context for Oxy Functions.**
  `createTestContext(manifest, { function, databases })` returns an
  `OxyFunctionContext` backed by in-memory stores that refuses exactly where the
  host refuses, in the host's own words: the manifest's capability gates
  (`secrets.write`, `email.send`, `org.read`, `storage.read` / `storage.write`,
  `oltp`, `airhouse`), the `destinations` allowlist and the customer-warehouse
  rule, `ctx.tx` on a non-Postgres database, `warehouse.upsert` where
  `ON CONFLICT` does not parse, `ctx.fetch`'s SSRF allowlist and byte cap. Every
  refusal is quoted from the host's source and held there by a Rust drift test,
  so a reworded refusal reaches an app's tests on its next SDK bump rather than
  never. Reads answer from a byte copy of the platform's shape zoo, so a
  `UInt64` above `i64::MAX` arrives as a string in a test as it does in
  production; `t.state.<store>.zoo()`, `.table(name, columns)` and `.raw(name,
  rows)` seed rows, the last marked `source: "author"` on every read of it.
  `t.calls`, `t.ops()`, `t.callsTo(op)` and `t.override(op, impl)` are typed by
  the host's closed `HostOp` list, and `t.run(fn)` evaluates a handler with the
  isolate's absent globals (`Buffer`, `TextEncoder`, `process`, …) removed.
  What it is not: a server, an engine, or a replacement for `oxyc checks run` —
  a test that passes here and fails there has found the host's words drifting
  from its behaviour, which is a platform bug to file. Spec and open questions:
  `internal-docs/sdk-testing-context.md`.

## [2.15.0] - 2026-09-23

### Added

- **`OxyFunctionRequest` declares `method` and `headers`.** The host has always
  sent `{ method, headers, body }` (`req_json` in `runtime.rs`); the type named
  only `body`, so the webhook-verification example in the functions guide —
  `req.headers["x-hub-signature-256"]` — was a type error against the SDK that
  documents it. Header names are keyed lower-case, a header sent twice collapses
  to its first value, and a scheduled or Airway run reports `POST` with `{}`.
  Nothing about the runtime value changed.

## [2.14.0] - 2026-09-23

### Added

- **`ctx.crypto` is declared on `OxyFunctionContext`.** The host has bound
  `ctx.crypto.hmac`, `ctx.crypto.verifyHmac` and `ctx.crypto.timingSafeEqual`
  since they shipped, and the authoring guide documents them, but the context
  type never declared them — so the documented webhook-verification call was a
  type error for a thing that works. `OxyCryptoApi`, `OxyHmacInput` and
  `OxyVerifyHmacInput` are exported. Typed from `__buildCtx` in `runtime.rs`:
  synchronous (no `await`);
  `algorithm` defaults to `"sha256"`, `encoding` to `"hex"`; `key` and `data`
  are required strings; `signature` and both sides of `timingSafeEqual` accept
  an absent value, because the host answers `false` for one rather than
  throwing.

### Fixed

- **`ctx.query` resolves to `{ rows, truncated }`, as the host has always sent
  it.** The type said `OxyFunctionRow[]`. A function that indexed or iterated
  the result compiled and failed on its first call (`rows.map is not a
  function`; `.length` reading `undefined`), while the docs, the scaffold and
  the platform canary already destructure `rows`. If you typed against the old
  shape, change `const rows = await ctx.query(sql)` to
  `const { rows } = await ctx.query(sql)` — the runtime value is unchanged.

## [2.13.1] - 2026-09-22

### Fixed

- **An aborted request no longer strands a fetching hook on `loading: true`.**
  `useSemanticQuery`, every metric-tree hook, `useWorldModelGraph`,
  `useWorldModelInstances`, `useMeasureBreakdown`, `useProcedureRun` and
  `useAgentRun` returned early on any `AbortError`, matched by name. Their own
  teardown is already marked (the per-run `cancelled` flag, or the run's
  `AbortController`), so the name check only ever fired for an abort the hook
  did NOT cause — a fetcher carrying its own timeout, a dev-proxy socket drop,
  a navigation. That read as `loading: true, error: null` for the rest of the
  page's life: skeletons with nothing to explain them, and a runner parked on
  `"running"`. Silence is now decided by whether we tore the run down; every
  other abort is reported as the failure it is.
- **An abort surfaced this way no longer reads as "The user aborted a
  request."** — no user did. It is reported as "request was interrupted (the
  connection dropped or timed out)", carrying the original as `cause`. The
  error's name is used only to word an error already being reported; whether
  to report at all is still decided by the hook's own teardown.
- **`useAgentRun`'s reconnect budget counts consecutive dead windows, not
  windows.** Five was the ceiling for the whole run, so a long stream that
  reconnects — a `fetcher` with its own request timeout, a proxy recycling the
  connection, an HTTP/2 `GOAWAY` — was reported `failed` after five windows
  even though every one of them delivered events. A window that made progress
  now starts the count over, whether it ended in a transport error or a
  graceful EOF; five windows that deliver nothing still give up. The server
  resumes from `Last-Event-ID`, so a reconnect never re-delivers and progress
  always means new events.
- **`refetch()` works on the metric-tree and world-model hooks.** The nonce it
  bumps was missing from the effect's dependency list, so every metric-tree
  hook plus `useWorldModelGraph` / `useWorldModelInstances` ignored it
  entirely — a panel whose read had failed could not be retried at all.
  `useMeasureBreakdown` has no `refetch` of its own: it is re-run by changing
  its `entityId` / `keyValue` / `measure`.

## [2.13.0] - 2026-09-09

### Added

- **`signOut(fetcher, links.login)` in `@oxy-hq/sdk/shell`, and `links.login`
  on the shell context.** An app ends its viewer's platform session with
  `GET /api/logout` and leaves for the product's login page with itself as
  `return_to`. On an enrolled tablet that page is "Who's on shift?", and the
  next PIN comes back into the app as that person; elsewhere it is the
  ordinary sign-in. `signOutUrl` is the pure half. A refused logout throws so
  the caller never navigates away from a session that still exists. The
  server names the login page because a custom-app subdomain cannot derive
  the product host.

## [2.12.0] - 2026-09-07

### Added

- **`ctx.org.places()` and `ctx.org.assignments()`.** The org's locations
  (hierarchy via `parent_id` and a tenant-named `kind`, lifecycle `status`,
  `timezone`, and `external_ids` — what Toast, a camera console or payroll
  call the place) and the roster (who holds which position where, and under
  whom). Same `org.read` capability as `people()`, same fail-closed refusal
  without it; `assignments()` is scoped to people who can reach the app, as
  `people()` is. Types `OxyOrgPlace` and `OxyOrgAssignment`. This is the
  platform half of "where you work decides what you see": an app reads the
  roster instead of keeping its own `staff` and `locations` tables. Design:
  `internal-docs/operating-graph.md`.
- **`ctx.user.reach`.** Where the caller may act, decided by the platform from
  their assignments before the function runs: `{ everywhere, via, locations }`
  — a system invocation, an app admin or a holder of an org-wide position
  everywhere; an assigned person exactly their places; an unassigned member
  everywhere and an unassigned frontline worker nowhere. A lookup failure
  lands on nowhere.
- **`@oxy-hq/sdk/ops`.** `reachOf`, `reaches`, `requireReach` (403
  `OutOfReach`), `predicate` (the WHERE fragment), `isAdmin`, `adminOnly` —
  Store Ops's `functions/access.ts` lifted into the SDK minus its roster SQL.
  Tighten, never widen: an absent reach reads as nowhere, including for a
  system invocation on a server without the graph.
- **Place in the semantic model.** A view's primary entity may declare
  `binding: { registry: locations, system: toast }`; with it,
  `useWorldModelInstances` items carry `location` (`{ id, name, kind,
  parent_id }`) when the key is mapped, `useWorldModelInstances(id, { scope:
  "reach" })` keeps only the instances the viewer reaches (`withheld` counts
  the rest), and `ctx.semantic.query({ ..., scope: "reach" })` — and the
  browser semantic query with `scope: "reach"` in its body — pins the query to
  the caller's places server-side. Toast order events now carry `location_id`
  / `location_name` when the restaurant GUID is mapped.
- **`useIdentity()`** (`@oxy-hq/sdk/shell`): the viewer in one shape on every
  server version — `id`, `name`, `email` (`null` for the crew), `picture`,
  `kind`, display-only `reach`. The shell context's `user` gains `id`, `kind`
  and `reach` on servers that carry the graph.

## [2.11.0] - 2026-09-06

Scenario forecasting: three metric-tree hooks for the what-if canvas, split by
what each one costs — documented in `docs/build/sdk/hooks.mdx` under "Scenario
forecasting".

### Added

- **`useBaseline` / `client.metricTree.getBaseline`** — values every node
  reachable from a set of levers over a window, and fits a coefficient for any
  driver edge whose `.view.yml` declares none.
- **`useProjection` / `client.metricTree.getProjection`** — bucketed history
  for the levers and everything downstream, plus the forecaster's forward
  curve. Returns the baseline curve only: the scenario's second curve is
  composed client-side from this and a `usePredict` result, so editing a lever
  never costs a query. A series carrying `refusal` (most often too little
  history to fit) is a stated absence, never a flat forward line.
- **`usePredict` / `predict()` take an options bag (`{ values, coefficients }`)**
  — feed `useBaseline`'s `values` and `fitted` straight through so an
  undeclared driver edge propagates instead of silently dropping its
  downstream measures from `impacts`. Existing calls with no second argument
  are unaffected.
- New exported types: `BaselineInstance`, `BaselineRequest`, `BaselineResponse`,
  `FittedDriver`, `ForecastPoint`, `HistoryPoint`, `MeasureProjection`,
  `MeasureValues`, `PredictOptions`, `ProjectionGranularity`,
  `ProjectionRequest`, `ProjectionResponse`, `UnvaluedNode`, `UsePredictOpts`.
- `DriverForm` grows five shapes (`quadratic`, `cubic`, `sqrt`, `inverse`,
  `linear-log-quadratic`) to match what airlayer can already emit — a tree
  carrying one of these edges no longer types as a value the union couldn't
  hold.
- `MetricEdge.operator` (`"add" | "sub" | "mul" | "div"`, absent means `add`)
  and `MetricEdge.form_declared` (whether the shape was authored in YAML or
  inferred by the fit).
- `MetricNode.drillable` — whether a measure can be drilled into; serialized
  rather than re-derived, since `measure_type` alone misses eligible
  composites.
- `MetricTree.warnings` — refusals raised while building the tree, e.g. a
  driver declaring both `coefficient:` and `coefficients:`.
- **`useTrack` events name their app.** `useOxyApp()` now returns `appId`
  (from `window.__OXY_APP__`), and each tracked event carries it as `app_id`.
  The events endpoint is keyed by workspace, and a workspace can publish
  several apps — without this the server attributed an event to *an* app in
  the workspace. Bundles built against an older SDK keep working; the server
  falls back to the old lookup when `app_id` is absent.

### Changed

- **`ctx.user.email` is `string | null`.** It was typed `string`; the runtime
  has sent `null` for a frontline worker — a crew member enrolled by PIN, with
  no mailbox — since workers existed. Source-breaking for an app that hands
  `ctx.user.email` to something that wants a `string` (the `functions`
  scaffold's `notify.ts` did, and now answers 400 when there is nobody to send
  to). That null is the one field that tells the crew from the office inside a
  function.

## [2.10.0] - 2026-09-03

### Changed

- **A function's HTTP status now reaches the caller.** `readFunctionSseStream`
  throws a `FunctionStatusError` when a function returns a non-2xx, carrying
  `status`, the parsed `body`, and the logs captured before it. Previously the
  server hardcoded `{"status": 200}` in the terminal frame and dropped the
  isolate's status, so a handler answering 400, 403 or 409 resolved as an
  ordinary success and every caller had to infer rejection from the body's
  shape.

  **This is a behaviour change.** An app that reads `{ error: … }` out of a
  resolved value will now see a rejection instead:

  ```ts
  try {
    await invoke({ pathId });
  } catch (e) {
    const err = e as FunctionError;
    if (err.status === 409) { /* everyone already holds it */ }
  }
  ```

  Two things keep it from breaking apps that do not return a non-2xx: a missing
  or non-numeric status is treated as success, so an app talking to a server
  that predates this is unaffected; and the check is a 2xx range rather than
  `!== 200`, so 201 and 204 stay successes.

  Requires an oxy server carrying the matching change; against an older one the
  behaviour is unchanged.

### Added

- `FunctionError`, `FunctionLog` and `FunctionResult` are exported. They were
  the SSE reader's types and had never been re-exported, so a consumer could not
  name the value a `catch` gives them.

## [2.9.1] - 2026-08-21

Review corrections to 2.9.0, which had not shipped yet — folded in rather than
left as a footnote, since the wording they fix is the contract itself.

- **`kind: "system"` also covers a manual *Run now***, not just cron and Airway.
  2.9.0 documented `"system"` as "no human behind it"; an operator clicking Run
  now in the admin console is a human, and the job trigger deliberately routes it
  down the same owner-identity path. The field now means *no caller to attribute
  this to* — which is the honest claim, and the one a function can act on. The
  triggering operator is still unreachable; carrying them through the task
  payload is a follow-up on the server.
- `ctx.user.teams` is now sorted **case-insensitively**. Byte order put every
  lowercase team name after every capitalised one, which renders as unsorted.

## [2.9.0] - 2026-08-21

Fills out `ctx.user`. A function could previously answer "who is calling" with an
id, an email, and a per-app role — enough to gate on, not enough to greet, label,
or explain with, and with no way to tell a human apart from a cron tick.

- **`ctx.user.name` / `ctx.user.picture`** — display identity, server-side. The
  workaround was passing them up from the client, which is forgeable and so
  defeats the point of reading identity from `ctx` at all.
- **`ctx.user.orgRole`** (`"owner" | "admin" | "member"`) — their standing in the
  owning org, absent for a staff break-glass visitor. Informational by design:
  gate on `appRole`, use this to explain ("ask your org admin to connect a
  warehouse"), label, or route.
- **`ctx.user.teams`** — the org teams they belong to, name-sorted and scoped to
  this app's org. Descriptive: a team confers something on an app only through an
  app team grant, which `appRole` already folds in.
- **`ctx.user.kind`** (`"user" | "system"`) — the supported way to tell a routed
  invocation from a background one. `"system"` means *no caller to attribute
  this to*, which also covers an operator's manual **Run now**: those run under
  the org owner's id with every caller field absent, so they still read
  `appRole: "admin"`, and the triggering operator is not reachable. The
  email-sniffing check this replaces (`endsWith("@system.oxy")`) was never a
  contract.
- **Fixed: `ctx.user.orgId` was `undefined`.** The host serialized the field as
  `org_id` while these types and every doc said `orgId` — so a tenant filter
  written against it compared against nothing. The host now sends `orgId`, and
  the runtime keeps `org_id` populated so functions written against the shipped
  behaviour keep working. **If a function of yours filters SQL on the org id,
  re-read that line.**
- New exported types: `OxyOrgTeam`, `OxyIdentityKind`.
- `teams` and `kind` are **optional** on the type. A server older than this
  release doesn't send them, and a required type would have promised otherwise:
  `ctx.user.teams.some(...)` throws there, and `kind === "system"` silently reads
  `false` — routing a cron tick down the human branch, the exact bug `kind`
  exists to prevent. There is no safe inference for an absent `kind` in either
  direction; mark the schedule's `input` if you must support one.

Full contract, including what is deliberately withheld from apps:
`internal-docs/custom-apps-user-identity.md`.

## [2.8.0] - 2026-08-02

Makes email attachments actually reachable from an Oxy Function. `content` was
documented as base64, but the Functions isolate is bare `deno_core` — no
`deno_web`, and a V8 predating `Uint8Array.prototype.toBase64` — so `btoa` was
`undefined` and there was no way to produce that base64 at all.

- **`bytesToBase64` / `base64ToBytes`** — new named exports for binary. Plain
  bundled JS, so they behave identically in the isolate, in Node/vitest, and in
  a browser; and because they take bytes, `bytesToBase64(u8)` cannot be misread
  the way `btoa(u8)` is (the spec stringifies it to `"37,80,68,70"` and encodes
  that text instead of your file).
- `btoa` / `atob` now exist in the isolate (polyfilled in the runtime
  bootstrap), following WHATWG semantics so a helper unit-tested under Node
  behaves the same there. Passing bytes to `btoa` throws and points at
  `bytesToBase64`. These globals are declared for the functions tsconfig only —
  deliberately **not** added to this package's browser type graph, where `btoa`
  is the real DOM one with no polyfill behind it.
- `EmailAttachment.encoding` (`"base64"` default, or `"utf8"`) — attach
  generated text (CSV/JSON/HTML) with no encoder and byte-exact for non-ASCII.
- `ctx.fetch` takes `encoding: "base64"` (`OxyFetchInit`) and echoes it on the
  result. The default UTF-8 decode is lossy, so fetching a binary to attach
  previously returned a silently corrupt file.

## [2.7.0] - 2026-07-27

Completes the anomaly inbox for standalone custom apps. `scan()` and
`explain()` were already in the SDK but 404'd against `/external/api` —
the surface a bundle served from its own origin has to use — because only
the read half of the inbox was mounted there. Both verbs are now exposed,
so `client.anomalies.*` behaves the same from a custom app as from the IDE.

### Added

- **`explain(id, { refresh: true })`** — busts the cached `ExplainResult`
  and recomputes. The server already accepted `?refresh=true`; the option
  was simply unreachable from the SDK.
- **`ScanResponse.pending` and `ScanResponse.failures`** — a scan that
  outruns the 55 s synchronous window (or is debounced within 60 s of a
  previous one) returns `pending: true` with **all counts zeroed**. Those
  zeros are not a "nothing found" result; check `pending` before reading
  them, then refetch with `list()`. `failures` carries per-monitor errors.
- **`Anomaly.dimension_key` / `Anomaly.filters`** — the segment a row
  belongs to. Already on the wire, previously absent from the type, so
  per-segment anomalies couldn't be told apart from chain-wide ones.
- **Exported types** `AnomalyFilter`, `ScanFailure`, `ExplainOptions`.

### Changed

- **`Anomaly.filters`, `ScanResponse.pending` and `ScanResponse.failures`
  are required-nullable, not optional.** The server serializes all three
  unconditionally, so the previous `?` overstated absence — `filters` is
  `null` for a chain-wide monitor and `failures` is `[]` on a clean scan,
  but neither is ever missing. Reading code gets strictly better types;
  code that **constructs** these objects (test fixtures, mocks) must now
  supply the fields.

## [2.6.0] - 2026-07-23

Adds a custom-app **asset store**, **email attachments**, and a
**per-app role** on `ctx.user` — three pieces of one story: an app can
now accept a file, keep it, show it back, email it, and restrict who
sees any of that.

### Added

- **`ctx.storage`** — the app's asset store, covering both kinds of file
  an app produces, in one per-app silo:
  - **Uploaded**: `getUploadUrl()` mints a presigned PUT and the browser
    uploads **straight to S3**, so uploads aren't bounded by the request
    body limit and the bytes never pass through the function or oxy.
  - **Generated**: `put(pathname, body, { encoding: "base64" })` writes a
    file the function itself produced — binary (PDF, PNG, Parquet) is
    first-class, not text-only.

    Full surface: `getUploadUrl`, `getDownloadUrl`, `put`, `get`, `head`,
    `list`, `delete`, `copy`. Gated by new fail-closed
    `storage: { read, write }` capabilities in `oxy-app.json`.

    Notable defaults, and why: **`allowOverwrite` is false** (silently
    clobbering an asset is worse than an error — enforced atomically via
    an S3 conditional write, not a racy check-then-put); **`list` is
    cursor-paginated** (a silo with 100k assets must not become one
    unbounded walk); **download links can live up to 7 days**, SigV4's own
    limit, because a link emailed to a human outlives a 15-minute upload
    window. Every asset is private — reads are always presigned and
    time-boxed; there is no public-access mode by design.

    Keys are confined to the calling app: another app's key is rejected on
    every operation, not just on read.

- **Email attachments** — `ctx.email.send({ attachments: [...] })` with
  `filename`, base64 `content`, optional `contentType`, and `inline` +
  `contentId` for `cid:`-referenced images. Max 20 per send and 10 MiB
  decoded in total; past that, store the file with `ctx.storage` and email
  a presigned link instead (which is the better shape anyway, since the
  file is usually retained regardless).

- **`ctx.user.appRole`** — `"admin"`, `"member"`, or absent, derived
  server-side from per-app membership (with org-owner / Oxy-staff
  break-glass). This is what a privileged in-app surface should gate on:
  unlike a query param or a client-side flag, the client cannot forge it.
  Deliberately *not* the org role — an app admin administers one app
  without holding org-Admin, which also carries billing and member
  management.

## [2.5.0] - 2026-07-22

Universalizes the custom-app shell: a bundle served from any origin can
now drive the wired shell (shell-context, Ask Oxygen) and theme its chrome
to match the host app.

### Added

- **`backendUrl` prop on `OxyAppProvider`** — the SDK resolves its relative
  `/api/*` requests against this origin, so a cross-origin bundle reaches
  the Oxy backend without a same-origin proxy. Opt-in: when unset, the
  fetcher is unchanged.
- **`chromeBackground` / `chromeForeground` props on `OxyShell`** — theme
  the rail + top bar + AskDock by overriding the host tokens
  (`--sidebar-background`, `--foreground`, `--muted-foreground`), which
  every shell sub-scope re-derives from.
- **AskDock history panel** — shows title + relative time ordered by
  `created_at`, keeps and highlights the active chat, and adds a search box
  and a "Show more" control.

### Removed

- **The built-in Settings rail item.** The shell adds no built-in rail
  entries — a bundle that wants a Settings link supplies it via
  `railBottom`.

### Fixed

- `dev` / `build:watch` now emit `dist/shell.css` too (tsdown `onSuccess`
  plus a CSS watcher), so live CSS edits are reflected instead of going
  stale until the next full `build`.

## [2.4.0] - 2026-07-21

Ships the Oxygen workspace shell — the same 48px icon rail + universal
top bar the main web-app renders — as a reusable subpath export, so a
custom app reads as part of the HQ. The web-app consumes these exact
components (oxygen-internal `internal-docs/2026-07-06-sdk-shell-universalization-design.md`).

### Added

- `@oxy-hq/sdk/shell` entry point: wired `OxyShell` + `useShellContext`
  (bootstraps from the new bundle-gated `GET /api/projects/:id/shell-context`;
  degrades to chrome-less rendering on older servers), and presentational
  `ShellRail`/`RailItem`, `TopBar`, `Breadcrumb`, `SystemIndicator`,
  `WorkspaceClock`, `WorkspaceTile`, `ShellTooltip`, `OxyMark`,
  `OxygenFactoryMark`, `workspaceLogoUrl`.
- `@oxy-hq/sdk/shell.css` — namespaced (`oxy-shell-*`) stylesheet; no
  Tailwind required. Follows host design tokens when present, falls back
  to the Oxygen defaults; dark mode via a `.dark` ancestor class.
- `useOxyApp()` is now exported (low-level identity + fetcher access
  without requiring the manifest to be ready).
- **Reasoning trace in the Ask dock** — the same trace the main web-app
  renders: header meta (LLM calls · total time · steps), step rows with
  status, indented tool rows with input previews + durations, streamed
  thinking text, and query rows with row counts. Expanded while
  streaming, auto-collapses when the run settles. Exported standalone as
  `ReasoningTrace` + `buildTraceSteps`/`aggregateLlmStats`.
- **Interactive charts in the Ask dock** — `AnswerChart` renders
  `chart_rendered` blocks with **ECharts** (the same library the main
  web-app uses) when the host app has it installed: axis/item tooltips,
  hover, legend, resize. `echarts` is an optional peer (dynamic import);
  bundles without it fall back to the dependency-free SVG render. Table
  type always uses the SVG/HTML table.
- **Native charts in the Ask dock** — `chart_rendered` blocks from the
  analytics pipeline render as dependency-free SVG (bar/line/pie, table
  fallback) via the exported `AnswerChart`; the duplicate QUERY artifact
  block is gone from the dock (the trace + chart carry that info).
- **API-backed chat history** — the Ask dock's History now lists the
  viewer's persistent threads from the bundle-gated
  `GET /api/projects/:id/threads`, merged with the richer in-session
  conversations. Opening a server thread fetches
  `GET /api/projects/:id/threads/:tid` and rebuilds the transcript (trace,
  charts, answer) by replaying the thread's persisted run events through
  the same processor the live stream uses; follow-ups resume the thread.
  Exported: `useThreadHistory`, `fetchThreadTranscript`. Requires an oxy
  server that stamps `user_id` on bundle threads (same release); older
  servers fall back to session-only history.
- **New chat + history in the Ask dock header** — a `＋` action starts a
  fresh conversation (new server thread) and a clock action opens
  session-scoped history: chats started in this dock session, each
  restorable (follow-ups resume its server thread via the SSE resume).
  Bundles have no list-threads endpoint, so history is per-session, not
  persisted across reloads.
- **Ask Oxygen dock** in the shell: when the app manifest binds an agent
  (`ask.agent`), the top bar shows the Ask Oxygen button (⌘K/Ctrl+K
  toggles) and a right-side chat dock opens as a flex sibling —
  compacting the app, not covering it. Multi-turn over `useAgentRun`
  (follow-ups reuse the thread), suggested-question chips from
  `ask.suggestedQuestions`, streamed answers with SQL artifacts, and an
  "open in Oxygen" header link to the product Chat surface. Also
  exported standalone as `AskDock`.
- The shell rail's bottom cluster shows a **Settings** entry linking to
  the product's Unified Settings Dialog (via the SPA's new
  `?settings=<section>` deep link, returned as `links.settings` by
  shell-context; hidden on servers that don't send it).

### Fixed

- `OxyAnswer` / `OxyChat` follow the host's design tokens when rendered
  inside the shell scope (dark mode included) — hardcoded light-theme
  colors remain only as fallbacks for standalone bundles. The spinner's
  `oxy-spin` keyframes are now actually defined (injected once), so it
  spins.

### Changed

- New runtime dependency: `@radix-ui/react-tooltip` (rail tooltips).
- New optional peer: `react-dom` (only needed by the shell's tooltip
  portal; data-only consumers are unaffected).

## [2.3.0] - 2026-07-20

### Added

- **Customer-app email sending** (oxy-internal `feat/customer-app-email-send`) —
  Oxy Functions can send email via `ctx.email.send({ to, subject, html|text, ... })`,
  backed by AWS SES. The **platform controls the `from` address**; the function
  sets `replyTo` only. Gated by a new fail-closed `email: { send: true }`
  capability on `OxyAppFunctionManifest`.
- **`@oxy-hq/sdk/email`** — a new subpath export shipping
  `render(Component, props)` (preact-render-to-string) so functions can author
  email bodies as **preact** components and render them to HTML inside the
  Functions isolate. `preact` / `preact-render-to-string` are optional peer
  deps, so the main SDK bundle is unaffected. (React Email / react-dom can't run
  in the isolate: its node build needs `node:stream`, its browser build needs
  Web Streams the isolate lacks.)
- **`OxyFunctionContext`** — the server-side function `ctx` is now typed
  (`user`, `env`, `log`, `query`, `queryStream`, `fetch`, `warehouse`, `secrets`,
  `semantic`, `airway`, `email`), exported from the custom-app entry.

## [2.2.0] - 2026-07-09

Publishes the custom-app **Oxy Functions** platform and the **metric-tree /
anomalies** clients that accumulated on `main` since 2.1.0. All additive — no
public export was removed or renamed.

### Added

- **Oxy Functions** (oxy-internal #2521) — `useFunction(name)` for invoking
  server-side TypeScript handlers shipped in a bundle's `functions/` dir, and the
  `functions` block on the app manifest (`route`, `timeoutSeconds`, `cache`,
  `destinations`).
- **Scheduled functions + secret writes** (#2685) — `schedule` (cron) +
  `timezone`, `airwayStep`, and `secrets: { write }` on `OxyAppFunctionManifest`,
  each with manifest validation. A function can now run on a cron schedule and
  persist app-scoped secrets via `ctx.secrets.set`.
- **Metric tree + anomalies** (#2407) — `MetricTreeClient` and `AnomaliesClient`
  and their result types (metric nodes/edges, driver attribution, anomaly
  list/scan).
- **Result caching + function SSE** (#2634) — client-side query-result cache
  (dedup + TTL) and the streaming transport for function invocations.

## [2.1.0] - 2026-06-08

Adds engineer-tagged usage events to the SDK so a bundle can record
which features its users actually exercise. Pairs with the per-app
**Activity** tab in the Oxy admin UI (oxy-internal #2465 / §13 of
`internal-docs/customer-apps.md`).

### Added

- `useTrackEvent()` — fire-and-forget hook returning
  `(name: string, payload?: object) => void`. Batches every 1s and
  flushes on `pagehide` via `sendBeacon` so a click that fires
  immediately before a navigation isn't lost. Server-validated
  `event_name` regex (`^[a-z][a-z0-9-]{0,63}$`) + 4 KiB payload cap
  + 60-events-per-minute rate limit per (user, app).

  ```tsx
  import { useTrackEvent } from "@oxy-hq/sdk";

  const track = useTrackEvent();
  <button
    onClick={() => {
      track("export-clicked", { format: "csv", rowCount });
      doExport();
    }}
  >Export</button>
  ```

### Notes

- View events (page loads) are recorded automatically by the oxy
  backend on every HTML serve — no SDK code required. The SDK hook
  is only for engineer-tagged interactions inside the bundle.
- Dev-mode caveat: `sendBeacon` carries cookies but bypasses the
  `OxyAppProvider` fetcher wrapper, so the `OXY_TOKEN` bearer
  injected by the vite-plugin proxy in cross-origin `pnpm dev` is
  missing on these requests. Cookie-served prod is unaffected.

## [2.0.0] - 2026-05-29

Complete rewrite: `@oxy-hq/sdk` is now a **React-only, customer-app-only**
SDK. A bundle wraps its tree in `<OxyAppProvider>` and reads from its linked
oxy project through hooks; identity is resolved from `oxy-app.json` +
`window.__OXY_APP__` (injected by oxy at serve time), and requests are
authenticated by the session cookie (same-origin) or a bearer token
(cross-origin dev).

### Added

- `OxyAppProvider` — resolves app identity and provides it via context.
- Hooks: `useQuery` (inline SQL), `useSemanticQuery` (semantic layer),
  `useAgentRun` (agent chat over SSE), `useProcedureRun` (long-running
  procedures, beta).
- Drop-in components: `<OxyChat>` and `<OxyAnswer>` (markdown + SQL
  artifacts; URL-scheme allowlist guards against `javascript:` injection).
- `OxyApiError` structured error envelope.
- Pairs with `@oxy-hq/vite-plugin` (base path, manifest copy, dev shim) and
  `create-oxy-app` scaffolding.

### Removed (BREAKING)

- The entire v1 stack: `OxyClient` / `OxySDK` / `OxyProvider`, the
  Parquet/DuckDB-WASM reader, and postMessage-based auth. Apps now talk to
  `/api/projects/:id/*` exclusively.
- `listApps` / `getAppData` / `runApp` / `getDisplays` / `getFile` /
  `getFileUrl`.

## [0.1.0] - 2025-01-01

### Added

- Initial release of the Oxy TypeScript SDK
- Core `OxyClient` with methods for app data fetching
- Configuration management with environment variable support
- Parquet file reading with DuckDB-WASM integration
- `ParquetReader` class for SQL queries on Parquet data
- Helper functions for quick Parquet data access
- Full TypeScript type definitions
- Comprehensive examples for Node.js, React, and v0 integration
- Documentation and API reference

### Features

- `listApps()` - List all apps in a project
- `getAppData()` - Fetch app data with caching
- `runApp()` - Run app and get fresh data
- `getDisplays()` - Get display configurations
- `getFile()` - Fetch files from state directory
- `getFileUrl()` - Get direct file URLs
- Parquet reading and SQL querying capabilities
- Support for both CommonJS and ES modules
- Browser and Node.js compatibility