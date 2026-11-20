// The shared recipes, as `.oxy-ui-*` class names: `ui.card`, `ui.btnCompact`, `ui.field`, …
//
// Translated from oxy-hq/reference-app@3caad1f `src/components/ui.ts` (1,252 lines of Tailwind
// class strings; its comments carry the reasons, and base.css repeats the ones a reader of the CSS
// needs), checked against Store Ops' copy (pokehouse-oxy `apps/store-ops/src/chrome/tokens.ts`),
// which is ahead of the reference in two places taken here: the focus ring reads the theme's ring
// colour, and the top-bar tab draws its ring inside the box (the bar clips one outside it).
//
// Nothing here is a component. These are the class names a component reaches for, so a select on
// one screen cannot drift from a select on another. Every name is styled in base.css, inside
// `@layer oxy-ui`, reading only theme.css tokens; no Tailwind is needed.
//
// **Two spellings, one set of classes.** Each reference export keeps its own name
// (`btnCompact`, `spacingSm`, `sheetRow`), so moving an app is `card` -> `ui.card`; and the plan's
// groups (`ui.btn.*`, `ui.space.*`, `ui.control.*`, `ui.table.*`, `ui.text.*`) name the same
// strings, for code written fresh. recipes.test.ts holds the two equal.
//
// **Density.** Compact (`controlSm`, `cell`, `page`) is records: a mouse, a desk, a reader
// comparing rows. Comfortable (`controlLg`, `touchRow`, `formPage`) is entry: a tablet, one task, a
// gloved finger, 44px targets and 16px fields. Only density and measure change between them; on
// a touch screen every desk control also reaches 44px (base.css, decision 16).

import { calloutTone, chipTone, toneFill, toneText } from "./tone";

const focusRing = "oxy-ui-focus";
const focusRingInset = "oxy-ui-focus-inset";

// Space: four gap steps, one per job (4 / 8 / 16 / 32), and two paddings.
const spacingXs = "oxy-ui-gap-xs";
const spacingSm = "oxy-ui-gap-sm";
const spacingMd = "oxy-ui-gap-md";
const spacingLg = "oxy-ui-gap-lg";
const paddingSm = "oxy-ui-pad-sm";
const paddingMd = "oxy-ui-pad-md";

// Heights: 32 desk, 36 a strip on its own row, 44 touch.
const heightSm = "oxy-ui-h-sm";
const heightMd = "oxy-ui-h-md";
const heightLg = "oxy-ui-h-lg";
const controlSm = "oxy-ui-control-sm";
const controlLg = "oxy-ui-control-lg";

// The type scale, named by size (36 / 24 / 20 / 16 / 14 / 13 / 12 / 11 / 10).
const text2xl = "oxy-ui-text-2xl";
const textXl = "oxy-ui-text-xl";
const textLg = "oxy-ui-text-lg";
const textBase = "oxy-ui-text-base";
const textMd = "oxy-ui-text-md";
const textSm = "oxy-ui-text-sm";
const textXs = "oxy-ui-text-xs";
const text2xs = "oxy-ui-text-2xs";
const text3xs = "oxy-ui-text-3xs";

// Buttons: three variants (primary, secondary, text) at two sizes (sm 32px, lg 44px).
const btnCompact = "oxy-ui-btn oxy-ui-btn--sm";
const btnCompactPrimary = "oxy-ui-btn--primary";
const btnCompactSecondary = "oxy-ui-btn--secondary";
const btnText = "oxy-ui-btn oxy-ui-btn--sm oxy-ui-btn--text";
const btnPrimary = "oxy-ui-btn oxy-ui-btn--lg oxy-ui-btn--primary";
const btnSecondary = "oxy-ui-btn oxy-ui-btn--lg oxy-ui-btn--secondary";
const btnTextLg = "oxy-ui-btn oxy-ui-btn--lg oxy-ui-btn--text";
/** Square, icon-only: the control height on both axes, no side padding. Needs an aria-label. */
const btnIconOnly = "oxy-ui-btn--icon";

// Fields: the comfortable 44px field (`input`), the compact one, the quiet search box.
const input = "oxy-ui-field";
const inputSm = "oxy-ui-field oxy-ui-field--sm";
const inputQuiet = "oxy-ui-field oxy-ui-field--quiet";
const errorInput = "oxy-ui-field--error";
const select = "oxy-ui-select";
const selectTouch = "oxy-ui-field oxy-ui-select--touch";
const selectChevron = "oxy-ui-select-chevron";
const selectPlaceholder = "oxy-ui-select-placeholder";

const cell = "oxy-ui-cell";
const numCell = "oxy-ui-cell oxy-ui-cell--num";
const sheetBand = "oxy-ui-sheet-band";
const sheetBandOffset = "oxy-ui-sheet-band-offset";
const sheetHeader = "oxy-ui-sheet-header";
const sheetRow = "oxy-ui-sheet-row";

const rowHover = "oxy-ui-row-hover";
const rowSelected = "oxy-ui-row-selected";

const tag = "oxy-ui-tag";

/**
 * Every recipe. Flat names are the reference's; the grouped objects at the end name the same
 * classes by family.
 */
export const ui = {
  // ── Focus ─────────────────────────────────────────────────────────────────────────────────
  /** The one focus treatment: a 1.5px ring in the theme's ring colour, keyboard focus only. */
  focusRing,
  /** The same ring drawn inside the box, for a control its container clips (a segment). */
  focusRingInset,

  // ── Space and layout ──────────────────────────────────────────────────────────────────────
  /** 4px: lines within one item, a name and its caption. */
  spacingXs,
  /** 8px: rows in a group, a panel's rows, an eyebrow to its content. */
  spacingSm,
  /** 16px: blocks on a page; the same size as `paddingMd`. */
  spacingMd,
  /** 32px: the sections of one record. */
  spacingLg,
  /** 12 x 8px: inside a boxed item (a figure tile, a reason, a ranked row). */
  paddingSm,
  /** 16px: inside a panel, a page, a record. */
  paddingMd,
  /** Every screen's outer frame: full width, `paddingMd` on every side. */
  page: "oxy-ui-page",
  /** The reading measure for prose, at any page width. */
  prose: "oxy-ui-prose",
  /** A record's sections, `spacingLg` apart; put `section` on each. */
  sections: "oxy-ui-sections",
  /** One section: its eyebrow and every row `spacingSm` apart. */
  section: "oxy-ui-section",
  /** An entry form's fixed narrow column (36rem), centred. */
  formPage: "oxy-ui-form-page",

  // ── Surfaces ──────────────────────────────────────────────────────────────────────────────
  /** A panel in the page: the card fill on a subtle edge. Border, never shadow. */
  card: "oxy-ui-card",
  /** A titled panel's geometry (pair with `card`): a column, `spacingSm`, `paddingMd`. */
  panel: "oxy-ui-panel",
  /** The rule under a panel's header, and the space it stands off by. */
  panelHeader: "oxy-ui-panel-header",
  /** A boxed item inside a panel: the field surface on a subtle edge. */
  tile: "oxy-ui-tile",

  // ── Type ──────────────────────────────────────────────────────────────────────────────────
  text2xl,
  textXl,
  textLg,
  textBase,
  textMd,
  textSm,
  textXs,
  text2xs,
  text3xs,
  /** A section's label: 12px caps in the subtle ink. */
  eyebrow: "oxy-ui-eyebrow",
  /** A muted 13px note that qualifies something above it. */
  note: "oxy-ui-note",
  /** A figure's second line in a sheet row: 11px, muted, tabular. */
  caption: "oxy-ui-caption",
  /** A field's label: 14px, medium, the dim ink. */
  label: "oxy-ui-label",
  /** A figure: the theme's figure role (20px, medium), tabular, full ink. */
  figure: "oxy-ui-figure",
  /** Digits on one width, for anything read as data. */
  tabularNums: "oxy-ui-tnum",

  // ── Controls ──────────────────────────────────────────────────────────────────────────────
  heightSm,
  heightMd,
  heightLg,
  /** 32px and 12px of side padding. */
  controlSm,
  /** 44px and 16px of side padding. */
  controlLg,

  // ── Buttons ───────────────────────────────────────────────────────────────────────────────
  /** The compact button's geometry; add a tone (`btnCompactPrimary` / `btnCompactSecondary`). */
  btnCompact,
  btnCompactPrimary,
  btnCompactSecondary,
  /** The text action: no fill, no border, the dim ink going to full ink on hover. */
  btnText,
  /** The 44px primary (entry screens). */
  btnPrimary,
  /** The 44px secondary. */
  btnSecondary,

  // ── Fields ────────────────────────────────────────────────────────────────────────────────
  /** The comfortable field: 44px, 16px text (iOS zooms under 16px). Inputs and textareas. */
  input,
  /** The compact field: 32px, a find box in a desk toolbar. */
  inputSm,
  /** The quiet search field: no edge until hovered or focused. Never a field to fill in. */
  inputQuiet,
  /** The error edge, appended by whoever holds the error (with `aria-invalid`). */
  errorInput,
  /** The compact select (32px), with the theme's chevron. */
  select,
  /** The comfortable select: `input` plus the chevron. */
  selectTouch,
  /** The chevron alone, for a caller drawing its own select. */
  selectChevron,
  /** A select showing its leading sentinel ("Select…") takes the placeholder's ink. */
  selectPlaceholder,

  // ── Status ────────────────────────────────────────────────────────────────────────────────
  /** The one small labelled mark: an 11px semibold tint. Add a `chipTone`. */
  tag,
  chipTone,
  /** A whole block that carries a status: one or two lines, 12px. Add a `calloutTone`. */
  callout: "oxy-ui-callout",
  calloutTone,
  toneText,
  toneFill,

  // ── Rows and selection ────────────────────────────────────────────────────────────────────
  rowHover,
  /** A selected row is a fill and nothing else. */
  rowSelected,
  /** Selection outranks hover: one or the other, never both. */
  rowState(selected: boolean): string {
    return selected ? rowSelected : rowHover;
  },
  /** The rule between list rows; none under the last. */
  rowRule: "oxy-ui-row-rule",

  // ── Tabs and segments ─────────────────────────────────────────────────────────────────────
  /** UNDERLINE tabs, which SCREEN: the row whose rule the selected tab breaks. */
  tabRow: "oxy-ui-tab-row",
  /** The parent strip's own row, full-bleed under the shell's top bar. */
  pageTabs: "oxy-ui-tab-row oxy-ui-page-tabs",
  /** The same strip riding in the shell's top bar; hidden until its caller shows it. */
  topBarTabs: "oxy-ui-top-bar-tabs",
  topBarTab: "oxy-ui-top-bar-tab",
  topBarTabOn: "oxy-ui-top-bar-tab--on",
  topBarTabOff: "oxy-ui-top-bar-tab--off",
  tab: "oxy-ui-tab",
  /** The selected tab: a folder tab, open into the panel below. */
  tabOn: "oxy-ui-tab--on",
  tabOff: "oxy-ui-tab--off",
  /** SEGMENTED, which SLICE of this screen: the track. */
  segmentRow: "oxy-ui-segment-row",
  segment: "oxy-ui-segment",
  segmentOn: "oxy-ui-segment--on",
  segmentOff: "oxy-ui-segment--off",
  /** A strip that shares its row with a 32px control drops to 32px. */
  segmentSharesRow: "oxy-ui-segment--shares-row",
  /** A count inside a tab or a segment: 12px semibold tabular on a tint. Add a `chipTone`, or: */
  tabCount: "oxy-ui-tab-count",
  tabCountNeutral: "oxy-ui-tab-count--neutral",
  /** The neutral count on a selected segment. */
  tabCountNeutralOn: "oxy-ui-tab-count--neutral-on",

  // ── Tables ────────────────────────────────────────────────────────────────────────────────
  /** A table cell: the BMG sheet's padding, top-aligned, the secondary ink. */
  cell,
  /** A figure cell: right-aligned, tabular. */
  numCell,
  /** A data sheet's group band (25px), header (40px) and body row (48px). */
  sheetBand,
  sheetBandOffset,
  sheetHeader,
  sheetRow,

  // ── Icons ─────────────────────────────────────────────────────────────────────────────────
  /** 14px: inside text, a tag or a chip. */
  iconInline: "oxy-ui-icon-inline",
  /** 16px: inside a 32px control. */
  iconControl: "oxy-ui-icon-control",

  // ── Navigation rows ───────────────────────────────────────────────────────────────────────
  /** A row in an app's Sections column: icon, then label, 36px. */
  sectionLink: "oxy-ui-section-link",
  sectionLinkOn: "oxy-ui-section-link--on",
  sectionLinkOff: "oxy-ui-section-link--off",
  /** One row of a touch list: the whole row is the 44px target. */
  touchRow: "oxy-ui-touch-row",

  // ── The same classes, by family ───────────────────────────────────────────────────────────
  btn: {
    primary: `${btnCompact} ${btnCompactPrimary}`,
    secondary: `${btnCompact} ${btnCompactSecondary}`,
    text: btnText,
    primaryLg: btnPrimary,
    secondaryLg: btnSecondary,
    textLg: btnTextLg,
    iconOnly: btnIconOnly
  },
  /** `ui.field` is `ui.input`, the plan's name for it. */
  field: input,
  fieldSm: inputSm,
  fieldQuiet: inputQuiet,
  fieldError: errorInput,
  space: {
    xs: spacingXs,
    sm: spacingSm,
    md: spacingMd,
    lg: spacingLg,
    paddingSm,
    paddingMd
  },
  control: {
    heightSm,
    heightMd,
    heightLg,
    sm: controlSm,
    lg: controlLg
  },
  table: {
    cell,
    numCell,
    band: sheetBand,
    bandOffset: sheetBandOffset,
    header: sheetHeader,
    row: sheetRow
  },
  text: {
    "2xl": text2xl,
    xl: textXl,
    lg: textLg,
    base: textBase,
    md: textMd,
    sm: textSm,
    xs: textXs,
    "2xs": text2xs,
    "3xs": text3xs
  },
  /** `ui.chip[tone]`: a tag in a tone, the gallery's Chip look. */
  chip: {
    success: `${tag} ${chipTone.success}`,
    warning: `${tag} ${chipTone.warning}`,
    error: `${tag} ${chipTone.error}`,
    info: `${tag} ${chipTone.info}`,
    neutral: `${tag} ${chipTone.neutral}`
  }
} as const;

export type UiRecipes = typeof ui;
